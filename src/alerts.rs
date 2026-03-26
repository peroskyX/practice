use chrono::{DateTime, Duration, Utc};

use crate::models::{
    AlertOperator, AlertRule, AlertSourceType, AlertStatus, LogEvent, MetricEvent,
};

pub fn evaluate_metric_window<'a>(
    rule: &AlertRule,
    service_name: &str,
    metrics: impl Iterator<Item = &'a MetricEvent>,
    now: DateTime<Utc>,
) -> Option<(f64, bool)> {
    if rule.source_type != AlertSourceType::Metric || !matches_service(rule, service_name) {
        return None;
    }

    let metric_name = rule.metric.as_ref()?;
    let window_start = now - Duration::seconds(rule.window_seconds as i64);

    let mut count = 0usize;
    let mut total = 0.0;

    for metric in metrics {
        if metric.service == service_name
            && metric.metric == *metric_name
            && metric.timestamp >= window_start
            && metric.timestamp <= now
        {
            count += 1;
            total += metric.value;
        }
    }

    if count == 0 {
        return Some((0.0, false));
    }

    let observed = total / count as f64;
    Some((observed, compare(observed, &rule.operator, rule.threshold)))
}

pub fn evaluate_log_window<'a>(
    rule: &AlertRule,
    service_name: &str,
    logs: impl Iterator<Item = &'a LogEvent>,
    now: DateTime<Utc>,
) -> Option<(f64, bool)> {
    if rule.source_type != AlertSourceType::Log || !matches_service(rule, service_name) {
        return None;
    }

    let window_start = now - Duration::seconds(rule.window_seconds as i64);
    let mut count = 0usize;

    for log in logs {
        if log.service == service_name
            && log.timestamp >= window_start
            && log.timestamp <= now
            && rule
                .level
                .as_ref()
                .map(|level| level == &log.level)
                .unwrap_or(true)
        {
            count += 1;
        }
    }

    let observed = count as f64;
    Some((observed, compare(observed, &rule.operator, rule.threshold)))
}

pub fn evaluate_heartbeat_window(
    rule: &AlertRule,
    service_name: &str,
    seconds_since_last_seen: f64,
) -> Option<(f64, bool)> {
    if rule.source_type != AlertSourceType::Heartbeat || !matches_service(rule, service_name) {
        return None;
    }

    Some((
        seconds_since_last_seen,
        compare(seconds_since_last_seen, &rule.operator, rule.threshold),
    ))
}

pub fn should_transition_to_firing(
    condition_started_at: DateTime<Utc>,
    now: DateTime<Utc>,
    window_seconds: u64,
) -> bool {
    now.signed_duration_since(condition_started_at) >= Duration::seconds(window_seconds as i64)
}

pub fn matches_service(rule: &AlertRule, service_name: &str) -> bool {
    rule.service == "*" || rule.service == service_name
}

pub fn compare(value: f64, operator: &AlertOperator, threshold: f64) -> bool {
    match operator {
        AlertOperator::GreaterThan => value > threshold,
        AlertOperator::LessThan => value < threshold,
        AlertOperator::GreaterThanOrEqual => value >= threshold,
        AlertOperator::LessThanOrEqual => value <= threshold,
        AlertOperator::Equal => (value - threshold).abs() < f64::EPSILON,
    }
}

pub fn build_alert_message(rule: &AlertRule, service_name: &str, observed: f64) -> String {
    let source = match rule.source_type {
        AlertSourceType::Metric => rule.metric.clone().unwrap_or_else(|| "metric".to_string()),
        AlertSourceType::Log => rule
            .level
            .as_ref()
            .map(|level| format!("{level:?} logs"))
            .unwrap_or_else(|| "logs".to_string()),
        AlertSourceType::Heartbeat => "heartbeat".to_string(),
    };

    format!(
        "Rule '{}' matched for service '{}' on {} (observed {:.2}, threshold {:?} {:.2})",
        rule.name, service_name, source, observed, rule.operator, rule.threshold
    )
}

pub fn is_active(status: &AlertStatus) -> bool {
    matches!(status, AlertStatus::Pending | AlertStatus::Firing)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use crate::models::{
        AlertOperator, AlertRule, AlertSeverity, AlertSourceType, LogEvent, LogLevel, MetricEvent,
    };

    use super::{evaluate_log_window, evaluate_metric_window, should_transition_to_firing};

    #[test]
    fn metric_window_uses_average_over_window() {
        let now = Utc::now();
        let rule = AlertRule {
            id: Uuid::new_v4(),
            name: "high latency".into(),
            service: "auth".into(),
            source_type: AlertSourceType::Metric,
            metric: Some("request_latency_ms".into()),
            level: None,
            operator: AlertOperator::GreaterThan,
            threshold: 500.0,
            window_seconds: 60,
            severity: AlertSeverity::High,
            enabled: true,
        };

        let metrics = vec![
            MetricEvent {
                id: Uuid::new_v4(),
                service: "auth".into(),
                metric: "request_latency_ms".into(),
                value: 700.0,
                timestamp: now,
                tags: Default::default(),
            },
            MetricEvent {
                id: Uuid::new_v4(),
                service: "auth".into(),
                metric: "request_latency_ms".into(),
                value: 650.0,
                timestamp: now,
                tags: Default::default(),
            },
        ];

        let (observed, matched) =
            evaluate_metric_window(&rule, "auth", metrics.iter(), now).expect("rule should apply");

        assert!(matched);
        assert!(observed > 600.0);
    }

    #[test]
    fn log_window_counts_matching_logs() {
        let now = Utc::now();
        let rule = AlertRule {
            id: Uuid::new_v4(),
            name: "error burst".into(),
            service: "*".into(),
            source_type: AlertSourceType::Log,
            metric: None,
            level: Some(LogLevel::Error),
            operator: AlertOperator::GreaterThanOrEqual,
            threshold: 2.0,
            window_seconds: 60,
            severity: AlertSeverity::High,
            enabled: true,
        };

        let logs = vec![
            LogEvent {
                id: Uuid::new_v4(),
                service: "api".into(),
                level: LogLevel::Error,
                message: "boom".into(),
                timestamp: now,
                metadata: Default::default(),
            },
            LogEvent {
                id: Uuid::new_v4(),
                service: "api".into(),
                level: LogLevel::Error,
                message: "boom again".into(),
                timestamp: now,
                metadata: Default::default(),
            },
        ];

        let (observed, matched) =
            evaluate_log_window(&rule, "api", logs.iter(), now).expect("rule should apply");

        assert_eq!(observed, 2.0);
        assert!(matched);
    }

    #[test]
    fn firing_requires_condition_to_hold_for_window() {
        let started_at = Utc::now();
        let later = started_at + chrono::Duration::seconds(30);
        let much_later = started_at + chrono::Duration::seconds(61);

        assert!(!should_transition_to_firing(started_at, later, 60));
        assert!(should_transition_to_firing(started_at, much_later, 60));
    }
}
