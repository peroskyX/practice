# RustPulse Backend Architecture Plan

This document breaks the RustPulse backend into implementation phases and records the target architecture for each phase. The code in this repository follows this plan and focuses only on the backend.

## Phase 1: Platform Skeleton

- Build a single Rust service with `axum`, `tokio`, shared application state, structured errors, and JSON APIs under `/api/v1`.
- Define core domain models for services, metrics, logs, alert rules, and alert events.
- Establish a hot-path in-memory store first so ingestion, querying, and alert evaluation can be developed without blocking on infrastructure.
- Expose the base routes:
  - ingestion: `POST /api/v1/metrics`, `POST /api/v1/logs`, `POST /api/v1/heartbeat`
  - query: `GET /api/v1/services`, `GET /api/v1/services/:serviceId`, `GET /api/v1/services/:serviceId/metrics`, `GET /api/v1/logs`, `GET /api/v1/alerts`, `GET /api/v1/overview`
  - admin: `POST|GET|PATCH|DELETE /api/v1/alert-rules`
  - realtime: `GET /ws`
- Keep admin protection lightweight for now with a static bearer token so the backend remains easy to run locally.

## Phase 2: Ingestion and Realtime Hot Path

- Normalize incoming telemetry and upsert service registry entries on every metric, log, or heartbeat.
- Maintain rolling in-memory windows for metrics, logs, active alerts, and service liveness.
- Broadcast metric, log, alert, and service-status changes over a single WebSocket fanout channel.
- Use bounded retention in memory to keep the backend demo-friendly and predictable under load.

## Phase 3: Health and Alert Engine

- Track service health with `healthy`, `degraded`, and `offline` states derived from last-seen timestamps.
- Run a background housekeeping loop that reevaluates heartbeat-based status and alert rules.
- Support three v1 alert rule source types:
  - metric threshold
  - log count threshold
  - heartbeat timeout
- Model the alert lifecycle as `pending -> firing -> resolved`, storing lifecycle records and active state separately.

## Phase 4: Query Layer and Dashboard Read Models

- Provide read models shaped for an operations dashboard rather than mirroring raw storage tables.
- Support filtering by service, environment, metric name, log level, search text, status, severity, and time windows.
- Compute overview and service summaries from recent telemetry windows:
  - request rate
  - average latency
  - error rate
  - active alert count
  - health counts
- Aggregate metric time-series responses into resolution buckets for charts.

## Phase 5: Durable Storage and Production Hardening

- Replace or augment the in-memory event store with PostgreSQL persistence for metrics, logs, rules, services, and alert history.
- Split hot-path writes from historical reads so the live dashboard remains fast while the database provides retention and replay.
- Add migrations, background pruning/downsampling, and a persistence abstraction that keeps the API layer stable.
- Upgrade admin auth, add notifications, and externalize fanout if the system grows beyond a single node.

## Current Repository Scope

The implementation in this repository delivers the backend skeleton, hot-path ingestion, real-time fanout, service health tracking, alert evaluation, and dashboard-oriented query APIs. PostgreSQL persistence is intentionally left as the next backend phase so the current code stays runnable without extra infrastructure while still matching the planned architecture.
