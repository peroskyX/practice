use axum::{
    Json, Router,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, patch, post},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{
        AlertsQuery, CreateAlertRuleRequest, HeartbeatIngest, LogIngest, LogsQuery, MetricIngest,
        MetricQuery, OneOrMany, UpdateAlertRuleRequest,
    },
    state::SharedState,
    ws,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/v1/metrics", post(post_metrics))
        .route("/api/v1/logs", post(post_logs))
        .route("/api/v1/heartbeat", post(post_heartbeat))
        .route("/api/v1/services", get(list_services))
        .route("/api/v1/services/:service_id", get(get_service))
        .route(
            "/api/v1/services/:service_id/metrics",
            get(get_service_metrics),
        )
        .route("/api/v1/logs", get(get_logs))
        .route("/api/v1/alerts", get(get_alerts))
        .route("/api/v1/overview", get(get_overview))
        .route(
            "/api/v1/alert-rules",
            post(create_alert_rule).get(list_alert_rules),
        )
        .route(
            "/api/v1/alert-rules/:rule_id",
            patch(update_alert_rule).delete(delete_alert_rule),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws::serve(socket, state))
}

async fn post_metrics(
    State(state): State<SharedState>,
    Json(payload): Json<OneOrMany<MetricIngest>>,
) -> AppResult<Json<crate::models::IngestResponse>> {
    Ok(Json(state.ingest_metrics(payload.into_vec())?))
}

async fn post_logs(
    State(state): State<SharedState>,
    Json(payload): Json<OneOrMany<LogIngest>>,
) -> AppResult<Json<crate::models::IngestResponse>> {
    Ok(Json(state.ingest_logs(payload.into_vec())?))
}

async fn post_heartbeat(
    State(state): State<SharedState>,
    Json(payload): Json<HeartbeatIngest>,
) -> AppResult<Json<crate::models::Service>> {
    Ok(Json(state.ingest_heartbeat(payload)?))
}

async fn list_services(
    State(state): State<SharedState>,
) -> Json<Vec<crate::models::ServiceSummary>> {
    Json(state.list_services())
}

async fn get_service(
    State(state): State<SharedState>,
    Path(service_id): Path<Uuid>,
) -> AppResult<Json<crate::models::ServiceDetailsResponse>> {
    Ok(Json(state.get_service_details(service_id)?))
}

async fn get_service_metrics(
    State(state): State<SharedState>,
    Path(service_id): Path<Uuid>,
    Query(query): Query<MetricQuery>,
) -> AppResult<Json<crate::models::MetricSeriesResponse>> {
    Ok(Json(state.query_metrics(service_id, query)?))
}

async fn get_logs(
    State(state): State<SharedState>,
    Query(query): Query<LogsQuery>,
) -> Json<Vec<crate::models::LogEvent>> {
    Json(state.query_logs(query))
}

async fn get_alerts(
    State(state): State<SharedState>,
    Query(query): Query<AlertsQuery>,
) -> Json<Vec<crate::models::AlertEvent>> {
    Json(state.query_alerts(query))
}

async fn get_overview(State(state): State<SharedState>) -> Json<crate::models::OverviewResponse> {
    Json(state.overview())
}

async fn create_alert_rule(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(payload): Json<CreateAlertRuleRequest>,
) -> AppResult<Json<crate::models::AlertRule>> {
    require_admin(&headers, &state)?;
    Ok(Json(state.create_rule(payload)?))
}

async fn list_alert_rules(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> AppResult<Json<Vec<crate::models::AlertRule>>> {
    require_admin(&headers, &state)?;
    Ok(Json(state.list_rules()))
}

async fn update_alert_rule(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(payload): Json<UpdateAlertRuleRequest>,
) -> AppResult<Json<crate::models::AlertRule>> {
    require_admin(&headers, &state)?;
    Ok(Json(state.update_rule(rule_id, payload)?))
}

async fn delete_alert_rule(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    require_admin(&headers, &state)?;
    state.delete_rule(rule_id)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

fn require_admin(headers: &HeaderMap, state: &SharedState) -> AppResult<()> {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    if provided == Some(state.config().admin_token.as_str()) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}
