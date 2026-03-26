use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use parking_lot::RwLock;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    alerts::{
        build_alert_message, evaluate_heartbeat_window, evaluate_log_window,
        evaluate_metric_window, is_active, matches_service, should_transition_to_firing,
    },
    config::Config,
    error::{AppError, AppResult},
    models::{
        AlertEvent, AlertRule, AlertSourceType, AlertStatus, AlertsQuery, CreateAlertRuleRequest,
        HeartbeatIngest, IngestResponse, LogEvent, LogIngest, LogsQuery, MetricEvent, MetricIngest,
        MetricPoint, MetricQuery, MetricSeriesResponse, OverviewResponse, RealtimeEvent, Service,
        ServiceDetailsResponse, ServiceStatus, ServiceSummary, UpdateAlertRuleRequest,
    },
};

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<InnerState>,
}

struct InnerState {
    config: Config,
    store: RwLock<Store>,
    broadcaster: broadcast::Sender<RealtimeEvent>,
}

struct Store {
    services_by_name: HashMap<String, Service>,
    service_name_by_id: HashMap<Uuid, String>,
    metrics: VecDeque<MetricEvent>,
    logs: VecDeque<LogEvent>,
    alert_rules: HashMap<Uuid, AlertRule>,
    alerts: Vec<AlertEvent>,
    alert_index_by_id: HashMap<Uuid, usize>,
    rule_runtime: HashMap<AlertKey, RuleRuntime>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct AlertKey {
    rule_id: Uuid,
    service_name: String,
}

#[derive(Clone, Debug, Default)]
struct RuleRuntime {
    condition_started_at: Option<DateTime<Utc>>,
    active_alert_id: Option<Uuid>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            services_by_name: HashMap::new(),
            service_name_by_id: HashMap::new(),
            metrics: VecDeque::new(),
            logs: VecDeque::new(),
            alert_rules: HashMap::new(),
            alerts: Vec::new(),
            alert_index_by_id: HashMap::new(),
            rule_runtime: HashMap::new(),
        }
    }
}

impl SharedState {
    pub fn new(config: Config) -> Self {
        let (broadcaster, _) = broadcast::channel(512);
        Self {
            inner: Arc::new(InnerState {
                config,
                store: RwLock::new(Store::default()),
                broadcaster,
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn spawn_housekeeping(&self) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(state.inner.config.housekeeping_interval);
            loop {
                ticker.tick().await;
                state.run_housekeeping();
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.inner.broadcaster.subscribe()
    }

    pub fn ingest_metrics(&self, payloads: Vec<MetricIngest>) -> AppResult<IngestResponse> {
        let mut accepted = 0usize;
        let mut broadcasts = Vec::new();

        {
            let mut store = self.inner.store.write();
            for payload in payloads {
                validate_metric(&payload)?;
                let environment = payload
                    .tags
                    .get("env")
                    .cloned()
                    .unwrap_or_else(|| self.inner.config.default_environment.clone());

                if let Some(service) = touch_service(
                    &mut store,
                    &self.inner.config,
                    &payload.service,
                    &environment,
                    payload.timestamp,
                ) {
                    broadcasts.push(RealtimeEvent::ServiceStatusChanged(service));
                }

                let event = MetricEvent {
                    id: Uuid::new_v4(),
                    service: payload.service,
                    metric: payload.metric,
                    value: payload.value,
                    timestamp: payload.timestamp,
                    tags: payload.tags,
                };

                store.metrics.push_back(event.clone());
                while store.metrics.len() > self.inner.config.metric_retention {
                    store.metrics.pop_front();
                }

                broadcasts.push(RealtimeEvent::MetricUpdate(event.clone()));
                broadcasts.extend(self.evaluate_metric_rules(&mut store, &event, event.timestamp));
                accepted += 1;
            }
        }

        self.broadcast_all(broadcasts);
        Ok(IngestResponse { accepted })
    }

    pub fn ingest_logs(&self, payloads: Vec<LogIngest>) -> AppResult<IngestResponse> {
        let mut accepted = 0usize;
        let mut broadcasts = Vec::new();

        {
            let mut store = self.inner.store.write();
            for payload in payloads {
                validate_log(&payload)?;
                let environment = payload
                    .metadata
                    .get("env")
                    .cloned()
                    .unwrap_or_else(|| self.inner.config.default_environment.clone());

                if let Some(service) = touch_service(
                    &mut store,
                    &self.inner.config,
                    &payload.service,
                    &environment,
                    payload.timestamp,
                ) {
                    broadcasts.push(RealtimeEvent::ServiceStatusChanged(service));
                }

                let event = LogEvent {
                    id: Uuid::new_v4(),
                    service: payload.service,
                    level: payload.level,
                    message: payload.message,
                    timestamp: payload.timestamp,
                    metadata: payload.metadata,
                };

                store.logs.push_back(event.clone());
                while store.logs.len() > self.inner.config.log_retention {
                    store.logs.pop_front();
                }

                broadcasts.push(RealtimeEvent::LogUpdate(event.clone()));
                broadcasts.extend(self.evaluate_log_rules(&mut store, &event, event.timestamp));
                accepted += 1;
            }
        }

        self.broadcast_all(broadcasts);
        Ok(IngestResponse { accepted })
    }

    pub fn ingest_heartbeat(&self, payload: HeartbeatIngest) -> AppResult<Service> {
        if payload.service.trim().is_empty() {
            return Err(AppError::BadRequest("heartbeat service is required".into()));
        }

        let mut broadcasts = Vec::new();
        let service = {
            let mut store = self.inner.store.write();
            let service = touch_service(
                &mut store,
                &self.inner.config,
                &payload.service,
                &self.inner.config.default_environment,
                payload.timestamp,
            )
            .unwrap_or_else(|| {
                store
                    .services_by_name
                    .get(&payload.service)
                    .cloned()
                    .expect("service must exist after heartbeat touch")
            });
            broadcasts.push(RealtimeEvent::ServiceStatusChanged(service.clone()));
            service
        };

        self.broadcast_all(broadcasts);
        Ok(service)
    }

    pub fn list_services(&self) -> Vec<ServiceSummary> {
        let store = self.inner.store.read();
        let mut services: Vec<_> = store.services_by_name.values().cloned().collect();
        services.sort_by(|left, right| left.name.cmp(&right.name));
        services
            .into_iter()
            .map(|service| {
                let (current_request_rate, average_latency_ms, error_rate) =
                    derive_service_metrics(&store, &service.name, Utc::now());
                ServiceSummary {
                    service,
                    current_request_rate,
                    average_latency_ms,
                    error_rate,
                }
            })
            .collect()
    }

    pub fn get_service_details(&self, service_id: Uuid) -> AppResult<ServiceDetailsResponse> {
        let store = self.inner.store.read();
        let service_name = store
            .service_name_by_id
            .get(&service_id)
            .ok_or_else(|| AppError::NotFound(format!("service '{service_id}' not found")))?;
        let service = store
            .services_by_name
            .get(service_name)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("service '{service_id}' not found")))?;

        let latest_metrics = store
            .metrics
            .iter()
            .rev()
            .filter(|metric| metric.service == *service_name)
            .take(25)
            .cloned()
            .collect::<Vec<_>>();

        let recent_logs = store
            .logs
            .iter()
            .rev()
            .filter(|log| log.service == *service_name)
            .take(25)
            .cloned()
            .collect::<Vec<_>>();

        let active_alerts = store
            .alerts
            .iter()
            .filter(|alert| alert.service == *service_name && is_active(&alert.status))
            .cloned()
            .collect::<Vec<_>>();

        let (current_request_rate, average_latency_ms, error_rate) =
            derive_service_metrics(&store, service_name, Utc::now());

        Ok(ServiceDetailsResponse {
            service,
            active_alerts,
            recent_logs,
            latest_metrics,
            current_request_rate,
            average_latency_ms,
            error_rate,
        })
    }

    pub fn query_metrics(
        &self,
        service_id: Uuid,
        query: MetricQuery,
    ) -> AppResult<MetricSeriesResponse> {
        let store = self.inner.store.read();
        let service_name = store
            .service_name_by_id
            .get(&service_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("service '{service_id}' not found")))?;

        let metric_name = query
            .metric
            .clone()
            .ok_or_else(|| AppError::BadRequest("metric query parameter is required".into()))?;
        let from = query
            .from
            .unwrap_or_else(|| Utc::now() - ChronoDuration::minutes(15));
        let to = query.to.unwrap_or_else(Utc::now);
        if from > to {
            return Err(AppError::BadRequest(
                "metric 'from' must be before 'to'".into(),
            ));
        }

        let mut filtered = store
            .metrics
            .iter()
            .filter(|metric| {
                metric.service == service_name
                    && metric.metric == metric_name
                    && metric.timestamp >= from
                    && metric.timestamp <= to
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by_key(|metric| metric.timestamp);

        let points = bucketize_metrics(filtered, query.resolution.unwrap_or(0));

        Ok(MetricSeriesResponse {
            service_id,
            service_name,
            metric: metric_name,
            points,
        })
    }

    pub fn query_logs(&self, query: LogsQuery) -> Vec<LogEvent> {
        let store = self.inner.store.read();
        let mut logs = store
            .logs
            .iter()
            .filter(|log| {
                query
                    .service
                    .as_ref()
                    .map(|service| &log.service == service)
                    .unwrap_or(true)
                    && query
                        .level
                        .as_ref()
                        .map(|level| &log.level == level)
                        .unwrap_or(true)
                    && query
                        .q
                        .as_ref()
                        .map(|needle| {
                            let needle = needle.to_lowercase();
                            log.message.to_lowercase().contains(&needle)
                                || log.metadata.iter().any(|(key, value)| {
                                    key.to_lowercase().contains(&needle)
                                        || value.to_lowercase().contains(&needle)
                                })
                        })
                        .unwrap_or(true)
                    && query.from.map(|from| log.timestamp >= from).unwrap_or(true)
                    && query.to.map(|to| log.timestamp <= to).unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        logs.sort_by_key(|log| std::cmp::Reverse(log.timestamp));
        logs.truncate(query.limit.unwrap_or(100));
        logs
    }

    pub fn query_alerts(&self, query: AlertsQuery) -> Vec<AlertEvent> {
        let store = self.inner.store.read();
        let mut alerts = store
            .alerts
            .iter()
            .filter(|alert| {
                query
                    .service
                    .as_ref()
                    .map(|service| &alert.service == service)
                    .unwrap_or(true)
                    && query
                        .severity
                        .as_ref()
                        .map(|severity| &alert.severity == severity)
                        .unwrap_or(true)
                    && query
                        .status
                        .as_ref()
                        .map(|status| &alert.status == status)
                        .unwrap_or(true)
                    && query
                        .from
                        .map(|from| alert.triggered_at >= from)
                        .unwrap_or(true)
                    && query.to.map(|to| alert.triggered_at <= to).unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        alerts.sort_by_key(|alert| std::cmp::Reverse(alert.triggered_at));
        alerts
    }

    pub fn overview(&self) -> OverviewResponse {
        let store = self.inner.store.read();
        let now = Utc::now();
        let total_services = store.services_by_name.len();
        let healthy_services = store
            .services_by_name
            .values()
            .filter(|service| service.status == ServiceStatus::Healthy)
            .count();
        let degraded_services = store
            .services_by_name
            .values()
            .filter(|service| service.status == ServiceStatus::Degraded)
            .count();
        let offline_services = store
            .services_by_name
            .values()
            .filter(|service| service.status == ServiceStatus::Offline)
            .count();
        let active_alerts = store
            .alerts
            .iter()
            .filter(|alert| is_active(&alert.status))
            .count();

        let minute_ago = now - ChronoDuration::minutes(1);
        let request_rate_last_minute = store
            .metrics
            .iter()
            .filter(|metric| metric.metric == "request_count" && metric.timestamp >= minute_ago)
            .map(|metric| metric.value)
            .sum::<f64>();

        let latency_points = store
            .metrics
            .iter()
            .filter(|metric| {
                metric.metric == "request_latency_ms" && metric.timestamp >= minute_ago
            })
            .map(|metric| metric.value)
            .collect::<Vec<_>>();
        let average_latency_last_minute = average(&latency_points);

        let recent_errors_last_minute = store
            .logs
            .iter()
            .filter(|log| {
                log.level == crate::models::LogLevel::Error && log.timestamp >= minute_ago
            })
            .count();

        OverviewResponse {
            total_services,
            healthy_services,
            degraded_services,
            offline_services,
            active_alerts,
            request_rate_last_minute,
            average_latency_last_minute,
            recent_errors_last_minute,
        }
    }

    pub fn create_rule(&self, request: CreateAlertRuleRequest) -> AppResult<AlertRule> {
        validate_rule_request(&request.name, request.threshold, request.window_seconds)?;
        let rule = AlertRule {
            id: Uuid::new_v4(),
            name: request.name,
            service: request.service,
            source_type: request.source_type,
            metric: request.metric,
            level: request.level,
            operator: request.operator,
            threshold: request.threshold,
            window_seconds: request.window_seconds,
            severity: request.severity,
            enabled: request.enabled.unwrap_or(true),
        };

        let mut store = self.inner.store.write();
        store.alert_rules.insert(rule.id, rule.clone());
        Ok(rule)
    }

    pub fn list_rules(&self) -> Vec<AlertRule> {
        let store = self.inner.store.read();
        let mut rules = store.alert_rules.values().cloned().collect::<Vec<_>>();
        rules.sort_by(|left, right| left.name.cmp(&right.name));
        rules
    }

    pub fn update_rule(
        &self,
        rule_id: Uuid,
        request: UpdateAlertRuleRequest,
    ) -> AppResult<AlertRule> {
        let mut store = self.inner.store.write();
        let rule = store
            .alert_rules
            .get_mut(&rule_id)
            .ok_or_else(|| AppError::NotFound(format!("alert rule '{rule_id}' not found")))?;

        if let Some(name) = request.name {
            rule.name = name;
        }
        if let Some(service) = request.service {
            rule.service = service;
        }
        if let Some(source_type) = request.source_type {
            rule.source_type = source_type;
        }
        if let Some(metric) = request.metric {
            rule.metric = Some(metric);
        }
        if let Some(level) = request.level {
            rule.level = Some(level);
        }
        if let Some(operator) = request.operator {
            rule.operator = operator;
        }
        if let Some(threshold) = request.threshold {
            rule.threshold = threshold;
        }
        if let Some(window_seconds) = request.window_seconds {
            rule.window_seconds = window_seconds;
        }
        if let Some(severity) = request.severity {
            rule.severity = severity;
        }
        if let Some(enabled) = request.enabled {
            rule.enabled = enabled;
        }
        validate_rule_request(&rule.name, rule.threshold, rule.window_seconds)?;

        Ok(rule.clone())
    }

    pub fn delete_rule(&self, rule_id: Uuid) -> AppResult<()> {
        let mut store = self.inner.store.write();
        store
            .alert_rules
            .remove(&rule_id)
            .ok_or_else(|| AppError::NotFound(format!("alert rule '{rule_id}' not found")))?;
        store.rule_runtime.retain(|key, _| key.rule_id != rule_id);
        Ok(())
    }

    pub fn run_housekeeping(&self) {
        let now = Utc::now();
        let mut broadcasts = Vec::new();
        {
            let mut store = self.inner.store.write();
            let service_names = store.services_by_name.keys().cloned().collect::<Vec<_>>();
            for service_name in service_names {
                if let Some(service) = store.services_by_name.get_mut(&service_name) {
                    let next_status = compute_status(self.config(), service.last_seen_at, now);
                    if next_status != service.status {
                        service.status = next_status;
                        broadcasts.push(RealtimeEvent::ServiceStatusChanged(service.clone()));
                    }
                }
            }

            let rules = store.alert_rules.values().cloned().collect::<Vec<_>>();
            for rule in rules {
                if !rule.enabled || rule.source_type != AlertSourceType::Heartbeat {
                    continue;
                }

                let service_names = if rule.service == "*" {
                    store.services_by_name.keys().cloned().collect::<Vec<_>>()
                } else {
                    vec![rule.service.clone()]
                };

                for service_name in service_names {
                    let Some(service) = store.services_by_name.get(&service_name) else {
                        continue;
                    };
                    let seconds_since_last_seen = now
                        .signed_duration_since(service.last_seen_at)
                        .num_seconds()
                        .max(0) as f64;
                    if let Some((observed, matched)) =
                        evaluate_heartbeat_window(&rule, &service_name, seconds_since_last_seen)
                    {
                        broadcasts.extend(apply_rule_condition(
                            &mut store,
                            &rule,
                            &service_name,
                            observed,
                            matched,
                            now,
                        ));
                    }
                }
            }
        }

        self.broadcast_all(broadcasts);
    }

    fn evaluate_metric_rules(
        &self,
        store: &mut Store,
        metric: &MetricEvent,
        now: DateTime<Utc>,
    ) -> Vec<RealtimeEvent> {
        let rules = store.alert_rules.values().cloned().collect::<Vec<_>>();
        let mut broadcasts = Vec::new();

        for rule in rules {
            if !rule.enabled || !matches_service(&rule, &metric.service) {
                continue;
            }

            if let Some((observed, matched)) =
                evaluate_metric_window(&rule, &metric.service, store.metrics.iter(), now)
            {
                broadcasts.extend(apply_rule_condition(
                    store,
                    &rule,
                    &metric.service,
                    observed,
                    matched,
                    now,
                ));
            }
        }

        broadcasts
    }

    fn evaluate_log_rules(
        &self,
        store: &mut Store,
        log: &LogEvent,
        now: DateTime<Utc>,
    ) -> Vec<RealtimeEvent> {
        let rules = store.alert_rules.values().cloned().collect::<Vec<_>>();
        let mut broadcasts = Vec::new();

        for rule in rules {
            if !rule.enabled || !matches_service(&rule, &log.service) {
                continue;
            }

            if let Some((observed, matched)) =
                evaluate_log_window(&rule, &log.service, store.logs.iter(), now)
            {
                broadcasts.extend(apply_rule_condition(
                    store,
                    &rule,
                    &log.service,
                    observed,
                    matched,
                    now,
                ));
            }
        }

        broadcasts
    }

    fn broadcast_all(&self, events: Vec<RealtimeEvent>) {
        for event in events {
            let _ = self.inner.broadcaster.send(event);
        }
    }
}

fn validate_metric(payload: &MetricIngest) -> AppResult<()> {
    if payload.service.trim().is_empty() {
        return Err(AppError::BadRequest("metric service is required".into()));
    }
    if payload.metric.trim().is_empty() {
        return Err(AppError::BadRequest("metric name is required".into()));
    }
    if !payload.value.is_finite() {
        return Err(AppError::BadRequest("metric value must be finite".into()));
    }
    Ok(())
}

fn validate_log(payload: &LogIngest) -> AppResult<()> {
    if payload.service.trim().is_empty() {
        return Err(AppError::BadRequest("log service is required".into()));
    }
    if payload.message.trim().is_empty() {
        return Err(AppError::BadRequest("log message is required".into()));
    }
    Ok(())
}

fn validate_rule_request(name: &str, threshold: f64, window_seconds: u64) -> AppResult<()> {
    if name.trim().is_empty() {
        return Err(AppError::BadRequest("alert rule name is required".into()));
    }
    if !threshold.is_finite() {
        return Err(AppError::BadRequest(
            "alert rule threshold must be finite".into(),
        ));
    }
    if window_seconds == 0 {
        return Err(AppError::BadRequest(
            "alert rule window_seconds must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn touch_service(
    store: &mut Store,
    config: &Config,
    service_name: &str,
    environment: &str,
    seen_at: DateTime<Utc>,
) -> Option<Service> {
    let service = store
        .services_by_name
        .entry(service_name.to_string())
        .or_insert_with(|| {
            let service = Service {
                id: Uuid::new_v4(),
                name: service_name.to_string(),
                environment: environment.to_string(),
                status: ServiceStatus::Healthy,
                last_seen_at: seen_at,
                created_at: seen_at,
            };
            store
                .service_name_by_id
                .insert(service.id, service_name.to_string());
            service
        });

    let next_last_seen = if seen_at > service.last_seen_at {
        seen_at
    } else {
        service.last_seen_at
    };
    service.last_seen_at = next_last_seen;
    if !environment.is_empty() {
        service.environment = environment.to_string();
    }

    let next_status = compute_status(config, service.last_seen_at, Utc::now());
    if next_status != ServiceStatus::Healthy || service.status != ServiceStatus::Healthy {
        service.status = ServiceStatus::Healthy;
        return Some(service.clone());
    }
    if service.created_at == seen_at {
        return Some(service.clone());
    }
    None
}

fn compute_status(
    config: &Config,
    last_seen_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> ServiceStatus {
    let silence = now.signed_duration_since(last_seen_at);
    if silence
        >= ChronoDuration::from_std(config.offline_after)
            .unwrap_or_else(|_| ChronoDuration::seconds(30))
    {
        ServiceStatus::Offline
    } else if silence
        >= ChronoDuration::from_std(config.degraded_after)
            .unwrap_or_else(|_| ChronoDuration::seconds(15))
    {
        ServiceStatus::Degraded
    } else {
        ServiceStatus::Healthy
    }
}

fn derive_service_metrics(
    store: &Store,
    service_name: &str,
    now: DateTime<Utc>,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let window_start = now - ChronoDuration::minutes(1);
    let request_values = store
        .metrics
        .iter()
        .filter(|metric| {
            metric.service == service_name
                && metric.metric == "request_count"
                && metric.timestamp >= window_start
        })
        .map(|metric| metric.value)
        .collect::<Vec<_>>();
    let latency_values = store
        .metrics
        .iter()
        .filter(|metric| {
            metric.service == service_name
                && metric.metric == "request_latency_ms"
                && metric.timestamp >= window_start
        })
        .map(|metric| metric.value)
        .collect::<Vec<_>>();
    let error_values = store
        .metrics
        .iter()
        .filter(|metric| {
            metric.service == service_name
                && metric.metric == "error_count"
                && metric.timestamp >= window_start
        })
        .map(|metric| metric.value)
        .collect::<Vec<_>>();

    let request_rate = if request_values.is_empty() {
        None
    } else {
        Some(request_values.iter().sum())
    };
    let average_latency = average(&latency_values);
    let error_rate = match (request_rate, error_values.is_empty()) {
        (Some(requests), false) if requests > 0.0 => {
            Some(error_values.iter().sum::<f64>() / requests * 100.0)
        }
        _ => None,
    };
    (request_rate, average_latency, error_rate)
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn bucketize_metrics(events: Vec<MetricEvent>, resolution_seconds: u64) -> Vec<MetricPoint> {
    if resolution_seconds == 0 {
        return events
            .into_iter()
            .map(|event| MetricPoint {
                timestamp: event.timestamp,
                value: event.value,
            })
            .collect();
    }

    let mut buckets: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for event in events {
        let bucket =
            event.timestamp.timestamp() / resolution_seconds as i64 * resolution_seconds as i64;
        buckets.entry(bucket).or_default().push(event.value);
    }

    buckets
        .into_iter()
        .filter_map(|(timestamp, values)| {
            Some(MetricPoint {
                timestamp: DateTime::<Utc>::from_timestamp(timestamp, 0)?,
                value: values.iter().sum::<f64>() / values.len() as f64,
            })
        })
        .collect()
}

fn apply_rule_condition(
    store: &mut Store,
    rule: &AlertRule,
    service_name: &str,
    observed: f64,
    matched: bool,
    now: DateTime<Utc>,
) -> Vec<RealtimeEvent> {
    let key = AlertKey {
        rule_id: rule.id,
        service_name: service_name.to_string(),
    };
    let runtime = store.rule_runtime.entry(key.clone()).or_default();
    let mut broadcasts = Vec::new();

    if matched {
        let active_alert_id = match runtime.active_alert_id {
            Some(alert_id) => alert_id,
            None => {
                let alert = AlertEvent {
                    id: Uuid::new_v4(),
                    rule_id: rule.id,
                    service: service_name.to_string(),
                    status: AlertStatus::Pending,
                    severity: rule.severity.clone(),
                    message: build_alert_message(rule, service_name, observed),
                    triggered_at: now,
                    resolved_at: None,
                    observed_value: Some(observed),
                };
                let alert_id = alert.id;
                store.alert_index_by_id.insert(alert_id, store.alerts.len());
                store.alerts.push(alert);
                runtime.active_alert_id = Some(alert_id);
                runtime.condition_started_at = Some(now);
                alert_id
            }
        };

        if let Some(index) = store.alert_index_by_id.get(&active_alert_id).copied() {
            if let Some(alert) = store.alerts.get_mut(index) {
                alert.message = build_alert_message(rule, service_name, observed);
                alert.observed_value = Some(observed);

                if alert.status == AlertStatus::Pending
                    && runtime
                        .condition_started_at
                        .map(|started| {
                            should_transition_to_firing(started, now, rule.window_seconds)
                        })
                        .unwrap_or(false)
                {
                    alert.status = AlertStatus::Firing;
                    broadcasts.push(RealtimeEvent::AlertFired(alert.clone()));
                }
            }
        }
    } else {
        if let Some(active_alert_id) = runtime.active_alert_id.take() {
            if let Some(index) = store.alert_index_by_id.get(&active_alert_id).copied() {
                if let Some(alert) = store.alerts.get_mut(index) {
                    if alert.status != AlertStatus::Resolved {
                        alert.status = AlertStatus::Resolved;
                        alert.resolved_at = Some(now);
                        alert.observed_value = Some(observed);
                        broadcasts.push(RealtimeEvent::AlertResolved(alert.clone()));
                    }
                }
            }
        }
        runtime.condition_started_at = None;
    }

    broadcasts
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{Duration, Utc};

    use crate::models::{
        AlertOperator, AlertSeverity, AlertSourceType, CreateAlertRuleRequest, HeartbeatIngest,
        LogIngest, LogLevel, MetricIngest,
    };

    use super::SharedState;

    #[test]
    fn metric_alert_transitions_from_pending_to_firing() {
        let state = SharedState::new(Default::default());
        let rule = state
            .create_rule(CreateAlertRuleRequest {
                name: "high latency".into(),
                service: "auth".into(),
                source_type: AlertSourceType::Metric,
                metric: Some("request_latency_ms".into()),
                level: None,
                operator: AlertOperator::GreaterThan,
                threshold: 500.0,
                window_seconds: 60,
                severity: AlertSeverity::High,
                enabled: Some(true),
            })
            .expect("rule should be created");

        let now = Utc::now();
        state
            .ingest_metrics(vec![MetricIngest {
                service: "auth".into(),
                metric: "request_latency_ms".into(),
                value: 700.0,
                timestamp: now,
                tags: BTreeMap::new(),
            }])
            .expect("metric ingest should succeed");
        state
            .ingest_metrics(vec![MetricIngest {
                service: "auth".into(),
                metric: "request_latency_ms".into(),
                value: 710.0,
                timestamp: now + Duration::seconds(61),
                tags: BTreeMap::new(),
            }])
            .expect("metric ingest should succeed");

        let alerts = state.query_alerts(Default::default());
        let alert = alerts
            .iter()
            .find(|alert| alert.rule_id == rule.id)
            .expect("alert should exist");
        assert_eq!(alert.status, crate::models::AlertStatus::Firing);
    }

    #[test]
    fn logs_can_be_filtered_by_level_and_text() {
        let state = SharedState::new(Default::default());
        let now = Utc::now();
        state
            .ingest_logs(vec![
                LogIngest {
                    service: "orders".into(),
                    level: LogLevel::Error,
                    message: "payment timeout".into(),
                    timestamp: now,
                    metadata: BTreeMap::new(),
                },
                LogIngest {
                    service: "orders".into(),
                    level: LogLevel::Info,
                    message: "payment completed".into(),
                    timestamp: now,
                    metadata: BTreeMap::new(),
                },
            ])
            .expect("log ingest should succeed");

        let logs = state.query_logs(crate::models::LogsQuery {
            service: Some("orders".into()),
            level: Some(LogLevel::Error),
            q: Some("timeout".into()),
            from: None,
            to: None,
            limit: Some(10),
        });

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "payment timeout");
    }

    #[test]
    fn housekeeping_marks_silent_service_offline() {
        let state = SharedState::new(Default::default());
        let now = Utc::now() - Duration::seconds(45);
        state
            .ingest_heartbeat(HeartbeatIngest {
                service: "gateway".into(),
                timestamp: now,
            })
            .expect("heartbeat should register service");

        state.run_housekeeping();

        let services = state.list_services();
        let service = services
            .iter()
            .find(|service| service.service.name == "gateway")
            .expect("service should exist");
        assert_eq!(
            service.service.status,
            crate::models::ServiceStatus::Offline
        );
    }
}
