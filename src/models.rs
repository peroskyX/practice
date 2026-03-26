use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Healthy,
    Degraded,
    Offline,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AlertSourceType {
    Metric,
    Log,
    Heartbeat,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AlertOperator {
    #[serde(rename = ">")]
    GreaterThan,
    #[serde(rename = "<")]
    LessThan,
    #[serde(rename = ">=")]
    GreaterThanOrEqual,
    #[serde(rename = "<=")]
    LessThanOrEqual,
    #[serde(rename = "=")]
    Equal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AlertStatus {
    Pending,
    Firing,
    Resolved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    pub environment: String,
    pub status: ServiceStatus,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricEvent {
    pub id: Uuid,
    pub service: String,
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEvent {
    pub id: Uuid,
    pub service: String,
    pub level: LogLevel,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: Uuid,
    pub name: String,
    pub service: String,
    pub source_type: AlertSourceType,
    pub metric: Option<String>,
    pub level: Option<LogLevel>,
    pub operator: AlertOperator,
    pub threshold: f64,
    pub window_seconds: u64,
    pub severity: AlertSeverity,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub service: String,
    pub status: AlertStatus,
    pub severity: AlertSeverity,
    pub message: String,
    pub triggered_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub observed_value: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricIngest {
    pub service: String,
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogIngest {
    pub service: String,
    pub level: LogLevel,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatIngest {
    pub service: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(item) => vec![item],
            Self::Many(items) => items,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub service: String,
    pub source_type: AlertSourceType,
    pub metric: Option<String>,
    pub level: Option<LogLevel>,
    pub operator: AlertOperator,
    pub threshold: f64,
    pub window_seconds: u64,
    pub severity: AlertSeverity,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateAlertRuleRequest {
    pub name: Option<String>,
    pub service: Option<String>,
    pub source_type: Option<AlertSourceType>,
    pub metric: Option<String>,
    pub level: Option<LogLevel>,
    pub operator: Option<AlertOperator>,
    pub threshold: Option<f64>,
    pub window_seconds: Option<u64>,
    pub severity: Option<AlertSeverity>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MetricQuery {
    pub metric: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub resolution: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct LogsQuery {
    pub service: Option<String>,
    pub level: Option<LogLevel>,
    pub q: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlertsQuery {
    pub status: Option<AlertStatus>,
    pub severity: Option<AlertSeverity>,
    pub service: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IngestResponse {
    pub accepted: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricSeriesResponse {
    pub service_id: Uuid,
    pub service_name: String,
    pub metric: String,
    pub points: Vec<MetricPoint>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceSummary {
    pub service: Service,
    pub current_request_rate: Option<f64>,
    pub average_latency_ms: Option<f64>,
    pub error_rate: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceDetailsResponse {
    pub service: Service,
    pub active_alerts: Vec<AlertEvent>,
    pub recent_logs: Vec<LogEvent>,
    pub latest_metrics: Vec<MetricEvent>,
    pub current_request_rate: Option<f64>,
    pub average_latency_ms: Option<f64>,
    pub error_rate: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OverviewResponse {
    pub total_services: usize,
    pub healthy_services: usize,
    pub degraded_services: usize,
    pub offline_services: usize,
    pub active_alerts: usize,
    pub request_rate_last_minute: f64,
    pub average_latency_last_minute: Option<f64>,
    pub recent_errors_last_minute: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RealtimeEvent {
    MetricUpdate(MetricEvent),
    LogUpdate(LogEvent),
    AlertFired(AlertEvent),
    AlertResolved(AlertEvent),
    ServiceStatusChanged(Service),
}
