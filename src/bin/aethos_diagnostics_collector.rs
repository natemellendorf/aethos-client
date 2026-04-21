#[path = "../aethos_core/mod.rs"]
#[allow(dead_code)]
mod aethos_core;

use std::net::SocketAddr;
use std::time::Duration;

use aethos_core::diagnostics::{
    summarize_events, DiagnosticEvent, DiagnosticsEventIngestRequest,
    DiagnosticsEventIngestResponse, DiagnosticsRunCreateRequest, DiagnosticsRunRecord,
    DiagnosticsRunSummaryResponse, DiagnosticsTimelineResponse,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, SqlitePool};
use tokio::time::interval;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Parser)]
#[command(name = "aethos-diagnostics-collector")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:9774")]
    listen: String,
    #[arg(long, default_value = "sqlite://aethos-diagnostics.sqlite3")]
    database_url: String,
    #[arg(long, default_value_t = 72)]
    retention_hours: u64,
}

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
}

#[derive(Debug, Deserialize)]
struct TimelineQuery {
    item_id: Option<String>,
    peer_id: Option<String>,
    platform: Option<String>,
    event_type: Option<String>,
    component: Option<String>,
}

#[derive(Debug, FromRow)]
struct RunRow {
    run_id: String,
    app: String,
    platform: String,
    status: String,
    created_at_utc: String,
    expires_at_utc: Option<String>,
    scenario: Option<String>,
    test_case_id: Option<String>,
    metadata_json: Option<String>,
}

#[derive(Debug, FromRow)]
struct EventRow {
    schema_version: String,
    run_id: String,
    session_id: String,
    encounter_id: String,
    event_id: String,
    timestamp_utc: String,
    platform: String,
    app: String,
    build_sha: String,
    component: String,
    event_type: String,
    phase: String,
    result: String,
    peer_id: Option<String>,
    remote_peer_id: Option<String>,
    item_id: Option<String>,
    bearer: Option<String>,
    reason_code: Option<String>,
    message: Option<String>,
    fields_json: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let args = Args::parse();
    let pool = SqlitePool::connect(&args.database_url).await?;
    init_db(&pool).await?;
    let state = AppState { pool: pool.clone() };
    spawn_retention_task(pool, args.retention_hours);

    let app = Router::new()
        .route("/api/v1/diagnostics/runs", post(create_run))
        .route("/api/v1/diagnostics/events", post(ingest_events))
        .route("/api/v1/diagnostics/runs/{run_id}", get(get_run))
        .route(
            "/api/v1/diagnostics/runs/{run_id}/timeline",
            get(get_timeline),
        )
        .route(
            "/api/v1/diagnostics/runs/{run_id}/summary",
            get(get_summary),
        )
        .route("/health", get(health))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = args.listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "diagnostics collector listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("PRAGMA synchronous=NORMAL")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS diagnostics_runs (
            run_id TEXT PRIMARY KEY,
            app TEXT NOT NULL,
            platform TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at_utc TEXT NOT NULL,
            expires_at_utc TEXT,
            scenario TEXT,
            test_case_id TEXT,
            metadata_json TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS diagnostics_events (
            event_id TEXT PRIMARY KEY,
            schema_version TEXT NOT NULL,
            run_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            encounter_id TEXT NOT NULL,
            timestamp_utc TEXT NOT NULL,
            platform TEXT NOT NULL,
            app TEXT NOT NULL,
            build_sha TEXT NOT NULL,
            component TEXT NOT NULL,
            event_type TEXT NOT NULL,
            phase TEXT NOT NULL,
            result TEXT NOT NULL,
            peer_id TEXT,
            remote_peer_id TEXT,
            item_id TEXT,
            bearer TEXT,
            reason_code TEXT,
            message TEXT,
            fields_json TEXT,
            FOREIGN KEY(run_id) REFERENCES diagnostics_runs(run_id)
        )",
    )
    .execute(pool)
    .await?;
    for statement in [
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_run_id ON diagnostics_events(run_id)",
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_item_id ON diagnostics_events(item_id)",
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_peer_id ON diagnostics_events(peer_id)",
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_platform ON diagnostics_events(platform)",
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_event_type ON diagnostics_events(event_type)",
        "CREATE INDEX IF NOT EXISTS idx_diagnostics_events_timestamp ON diagnostics_events(timestamp_utc)",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }
    Ok(())
}

fn spawn_retention_task(pool: SqlitePool, retention_hours: u64) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(300));
        loop {
            ticker.tick().await;
            if let Err(err) = cleanup_expired(&pool, retention_hours).await {
                error!(error = %err, "retention cleanup failed");
            }
        }
    });
}

async fn cleanup_expired(pool: &SqlitePool, retention_hours: u64) -> Result<(), sqlx::Error> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(retention_hours as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    sqlx::query("DELETE FROM diagnostics_events WHERE timestamp_utc < ?")
        .bind(&cutoff)
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM diagnostics_runs WHERE expires_at_utc IS NOT NULL AND expires_at_utc < ?",
    )
    .bind(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    .execute(pool)
    .await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true}))
}

async fn create_run(
    State(state): State<AppState>,
    Json(request): Json<DiagnosticsRunCreateRequest>,
) -> Result<Json<DiagnosticsRunRecord>, (StatusCode, String)> {
    let run_id = request
        .requested_run_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("run-{}", chrono::Utc::now().timestamp_millis()));
    let created_at_utc = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let expires_at_utc = request.ttl_seconds.map(|ttl| {
        (chrono::Utc::now() + chrono::Duration::seconds(ttl as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
    sqlx::query(
        "INSERT OR REPLACE INTO diagnostics_runs (
            run_id, app, platform, status, created_at_utc, expires_at_utc, scenario, test_case_id, metadata_json
        ) VALUES (?, ?, ?, 'active', ?, ?, ?, ?, ?)",
    )
    .bind(&run_id)
    .bind(&request.app)
    .bind(&request.platform)
    .bind(&created_at_utc)
    .bind(expires_at_utc.as_deref())
    .bind(request.scenario.as_deref())
    .bind(request.test_case_id.as_deref())
    .bind(request.metadata.as_ref().map(Value::to_string))
    .execute(&state.pool)
    .await
    .map_err(internal_error)?;

    Ok(Json(DiagnosticsRunRecord {
        run_id,
        app: request.app,
        platform: request.platform,
        status: "active".to_string(),
        created_at_utc,
        expires_at_utc,
        scenario: request.scenario,
        test_case_id: request.test_case_id,
        metadata: request.metadata,
    }))
}

async fn ingest_events(
    State(state): State<AppState>,
    Json(request): Json<DiagnosticsEventIngestRequest>,
) -> Result<Json<DiagnosticsEventIngestResponse>, (StatusCode, String)> {
    let mut transaction = state.pool.begin().await.map_err(internal_error)?;
    let mut accepted = 0usize;
    for event in &request.events {
        sqlx::query(
            "INSERT OR REPLACE INTO diagnostics_events (
                event_id, schema_version, run_id, session_id, encounter_id, timestamp_utc,
                platform, app, build_sha, component, event_type, phase, result,
                peer_id, remote_peer_id, item_id, bearer, reason_code, message, fields_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.event_id)
        .bind(&event.schema_version)
        .bind(&event.run_id)
        .bind(&event.session_id)
        .bind(&event.encounter_id)
        .bind(&event.timestamp_utc)
        .bind(&event.platform)
        .bind(&event.app)
        .bind(&event.build_sha)
        .bind(&event.component)
        .bind(&event.event_type)
        .bind(&event.phase)
        .bind(&event.result)
        .bind(event.peer_id.as_deref())
        .bind(event.remote_peer_id.as_deref())
        .bind(event.item_id.as_deref())
        .bind(event.bearer.as_deref())
        .bind(event.reason_code.as_deref())
        .bind(event.message.as_deref())
        .bind(event.fields.as_ref().map(Value::to_string))
        .execute(&mut *transaction)
        .await
        .map_err(internal_error)?;
        accepted += 1;
    }
    transaction.commit().await.map_err(internal_error)?;
    Ok(Json(DiagnosticsEventIngestResponse {
        accepted,
        dropped: 0,
    }))
}

async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<DiagnosticsRunRecord>, (StatusCode, String)> {
    let row = sqlx::query_as::<_, RunRow>(
        "SELECT run_id, app, platform, status, created_at_utc, expires_at_utc, scenario, test_case_id, metadata_json
         FROM diagnostics_runs WHERE run_id = ?",
    )
    .bind(&run_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal_error)?
    .ok_or_else(not_found)?;
    Ok(Json(run_row_to_record(row)))
}

async fn get_timeline(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<DiagnosticsTimelineResponse>, (StatusCode, String)> {
    let mut sql = String::from(
        "SELECT schema_version, run_id, session_id, encounter_id, event_id, timestamp_utc,
            platform, app, build_sha, component, event_type, phase, result,
            peer_id, remote_peer_id, item_id, bearer, reason_code, message, fields_json
         FROM diagnostics_events WHERE run_id = ?",
    );
    if query.item_id.is_some() {
        sql.push_str(" AND item_id = ?");
    }
    if query.peer_id.is_some() {
        sql.push_str(" AND peer_id = ?");
    }
    if query.platform.is_some() {
        sql.push_str(" AND platform = ?");
    }
    if query.event_type.is_some() {
        sql.push_str(" AND event_type = ?");
    }
    if query.component.is_some() {
        sql.push_str(" AND component = ?");
    }
    sql.push_str(" ORDER BY timestamp_utc ASC, event_id ASC");

    let mut built = sqlx::query_as::<_, EventRow>(&sql).bind(&run_id);
    if let Some(item_id) = query.item_id.as_deref() {
        built = built.bind(item_id);
    }
    if let Some(peer_id) = query.peer_id.as_deref() {
        built = built.bind(peer_id);
    }
    if let Some(platform) = query.platform.as_deref() {
        built = built.bind(platform);
    }
    if let Some(event_type) = query.event_type.as_deref() {
        built = built.bind(event_type);
    }
    if let Some(component) = query.component.as_deref() {
        built = built.bind(component);
    }

    let rows = built.fetch_all(&state.pool).await.map_err(internal_error)?;
    Ok(Json(DiagnosticsTimelineResponse {
        run_id,
        events: rows.into_iter().map(event_row_to_event).collect(),
    }))
}

async fn get_summary(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<DiagnosticsRunSummaryResponse>, (StatusCode, String)> {
    let rows = sqlx::query_as::<_, EventRow>(
        "SELECT schema_version, run_id, session_id, encounter_id, event_id, timestamp_utc,
            platform, app, build_sha, component, event_type, phase, result,
            peer_id, remote_peer_id, item_id, bearer, reason_code, message, fields_json
         FROM diagnostics_events WHERE run_id = ? ORDER BY timestamp_utc ASC, event_id ASC",
    )
    .bind(&run_id)
    .fetch_all(&state.pool)
    .await
    .map_err(internal_error)?;
    let events = rows.into_iter().map(event_row_to_event).collect::<Vec<_>>();
    Ok(Json(summarize_events(&run_id, &events)))
}

fn run_row_to_record(row: RunRow) -> DiagnosticsRunRecord {
    DiagnosticsRunRecord {
        run_id: row.run_id,
        app: row.app,
        platform: row.platform,
        status: row.status,
        created_at_utc: row.created_at_utc,
        expires_at_utc: row.expires_at_utc,
        scenario: row.scenario,
        test_case_id: row.test_case_id,
        metadata: row
            .metadata_json
            .and_then(|value| serde_json::from_str::<Value>(&value).ok()),
    }
}

fn event_row_to_event(row: EventRow) -> DiagnosticEvent {
    DiagnosticEvent {
        schema_version: row.schema_version,
        run_id: row.run_id,
        session_id: row.session_id,
        encounter_id: row.encounter_id,
        event_id: row.event_id,
        timestamp_utc: row.timestamp_utc,
        platform: row.platform,
        app: row.app,
        build_sha: row.build_sha,
        component: row.component,
        event_type: row.event_type,
        phase: row.phase,
        result: row.result,
        peer_id: row.peer_id,
        remote_peer_id: row.remote_peer_id,
        item_id: row.item_id,
        bearer: row.bearer,
        reason_code: row.reason_code,
        message: row.message,
        fields: row
            .fields_json
            .and_then(|value| serde_json::from_str::<Value>(&value).ok()),
    }
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn not_found() -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, "run not found".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aethos_core::diagnostics::{DiagnosticEvent, DIAGNOSTICS_SCHEMA_VERSION};
    use serde_json::json;

    async fn test_state() -> AppState {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect sqlite memory db");
        init_db(&pool).await.expect("initialize schema");
        AppState { pool }
    }

    fn sample_event(
        event_id: &str,
        event_type: &str,
        phase: &str,
        item_id: Option<&str>,
    ) -> DiagnosticEvent {
        DiagnosticEvent {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION.to_string(),
            run_id: "run-test".to_string(),
            session_id: "session-test".to_string(),
            encounter_id: "encounter-test".to_string(),
            event_id: event_id.to_string(),
            timestamp_utc: "2026-04-21T20:15:12.123Z".to_string(),
            platform: "ios".to_string(),
            app: "aethos-ios".to_string(),
            build_sha: "abc1234".to_string(),
            component: "protocol.lan".to_string(),
            event_type: event_type.to_string(),
            phase: phase.to_string(),
            result: "ok".to_string(),
            peer_id: Some("peer-1".to_string()),
            remote_peer_id: None,
            item_id: item_id.map(|value| value.to_string()),
            bearer: Some("lan".to_string()),
            reason_code: None,
            message: None,
            fields: Some(json!({"sequence": 1})),
        }
    }

    #[tokio::test]
    async fn create_run_roundtrips_metadata() {
        let state = test_state().await;
        let response = create_run(
            State(state.clone()),
            Json(DiagnosticsRunCreateRequest {
                requested_run_id: Some("run-test".to_string()),
                app: "aethos-ios".to_string(),
                platform: "ios".to_string(),
                scenario: Some("clean".to_string()),
                test_case_id: Some("ios-clean".to_string()),
                metadata: Some(json!({"source": "unit-test"})),
                ttl_seconds: Some(60),
            }),
        )
        .await
        .expect("create run");

        assert_eq!(response.0.run_id, "run-test");
        assert_eq!(response.0.app, "aethos-ios");
        assert_eq!(response.0.metadata, Some(json!({"source": "unit-test"})));

        let fetched = get_run(State(state), Path("run-test".to_string()))
            .await
            .expect("fetch run");
        assert_eq!(fetched.0.test_case_id.as_deref(), Some("ios-clean"));
    }

    #[tokio::test]
    async fn summary_reports_missing_ui_projection_from_ingested_events() {
        let state = test_state().await;
        let _ = create_run(
            State(state.clone()),
            Json(DiagnosticsRunCreateRequest {
                requested_run_id: Some("run-test".to_string()),
                app: "aethos-ios".to_string(),
                platform: "ios".to_string(),
                scenario: None,
                test_case_id: None,
                metadata: None,
                ttl_seconds: None,
            }),
        )
        .await
        .expect("create run");

        let ingest = ingest_events(
            State(state.clone()),
            Json(DiagnosticsEventIngestRequest {
                events: vec![
                    sample_event("event-1", "request.sent", "request", Some("item-1")),
                    sample_event("event-2", "transfer.received", "transfer", Some("item-1")),
                    sample_event(
                        "event-3",
                        "inbox.import.succeeded",
                        "import",
                        Some("item-1"),
                    ),
                ],
            }),
        )
        .await
        .expect("ingest events");
        assert_eq!(ingest.0.accepted, 3);

        let summary = get_summary(State(state), Path("run-test".to_string()))
            .await
            .expect("get summary");
        assert_eq!(summary.0.highest_protocol_phase, "import");
        assert!(summary
            .0
            .missing_transitions
            .iter()
            .any(|item| item.missing == "ui.projection.succeeded"));
        assert_eq!(summary.0.item_ids_received, vec!["item-1".to_string()]);
    }

    #[tokio::test]
    async fn timeline_filters_by_item_id() {
        let state = test_state().await;
        let _ = create_run(
            State(state.clone()),
            Json(DiagnosticsRunCreateRequest {
                requested_run_id: Some("run-test".to_string()),
                app: "aethos-ios".to_string(),
                platform: "ios".to_string(),
                scenario: None,
                test_case_id: None,
                metadata: None,
                ttl_seconds: None,
            }),
        )
        .await
        .expect("create run");
        let _ = ingest_events(
            State(state.clone()),
            Json(DiagnosticsEventIngestRequest {
                events: vec![
                    sample_event("event-1", "transfer.received", "transfer", Some("item-1")),
                    sample_event("event-2", "transfer.received", "transfer", Some("item-2")),
                ],
            }),
        )
        .await
        .expect("ingest events");

        let timeline = get_timeline(
            State(state),
            Path("run-test".to_string()),
            Query(TimelineQuery {
                item_id: Some("item-2".to_string()),
                peer_id: None,
                platform: None,
                event_type: None,
                component: None,
            }),
        )
        .await
        .expect("get timeline");

        assert_eq!(timeline.0.events.len(), 1);
        assert_eq!(timeline.0.events[0].item_id.as_deref(), Some("item-2"));
    }
}
