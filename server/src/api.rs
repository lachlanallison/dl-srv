use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::config::{self, Config};
use crate::manager::Manager;
use crate::store::{AddTaskInput, RssFeed, Task};
use crate::version::HealthReport;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<tokio::sync::RwLock<Config>>,
    pub manager: Arc<Manager>,
    pub events: tokio::sync::broadcast::Sender<Task>,
    pub rate_limiter: Arc<Mutex<RateLimiter>>,
}

pub struct RateLimiter {
    limit: u32,
    hits: HashMap<String, Vec<Instant>>,
}

impl RateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            hits: HashMap::new(),
        }
    }

    fn check(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let hits = self.hits.entry(key.to_string()).or_default();
        hits.retain(|t| now.duration_since(*t) < window);
        if hits.len() as u32 >= self.limit {
            return false;
        }
        hits.push(now);
        true
    }
}

pub fn routes(state: AppState) -> Router {
    let public = Router::new().route("/setup", get(get_setup));

    let protected = Router::new()
        .route("/health", get(health))
        .route("/events", get(events_sse))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/{id}", get(get_task).delete(remove_task))
        .route("/tasks/{id}/pause", post(pause_task))
        .route("/tasks/{id}/resume", post(resume_task))
        .route("/settings", get(get_settings).put(update_settings))
        .route("/settings/regenerate-token", post(regenerate_token))
        .route("/setup", post(post_setup))
        .route("/binaries/ytdlp/update", post(update_ytdlp))
        .route("/rss/feeds", get(list_rss_feeds).post(create_rss_feed))
        .route(
            "/rss/feeds/{id}",
            get(get_rss_feed)
                .put(update_rss_feed)
                .delete(delete_rss_feed),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    public.merge(protected).with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let path = request.uri().path();
    if path.ends_with("/setup") && request.method() == axum::http::Method::GET {
        return Ok(next.run(request).await);
    }

    let token = state.cfg.read().await.token.clone();
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = auth == format!("Bearer {token}")
        || headers
            .get("X-Api-Token")
            .and_then(|v| v.to_str().ok())
            .map(|t| t == token)
            .unwrap_or(false);
    if ok {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn health(State(state): State<AppState>) -> Json<HealthReport> {
    match tokio::time::timeout(
        Duration::from_secs(25),
        state.manager.version_checker().health(),
    )
    .await
    {
        Ok(report) => Json(report),
        Err(_) => Json(HealthReport {
            dlsrv_version: env!("CARGO_PKG_VERSION").to_string(),
            aria2_ok: false,
            aria2_version: None,
            binaries: vec![],
            checked_at: chrono::Utc::now(),
        }),
    }
}

async fn events_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        msg.ok().and_then(|task| {
            serde_json::to_string(&task)
                .ok()
                .map(|json| Ok(Event::default().event("task").data(json)))
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    status: Option<String>,
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Task>>, StatusCode> {
    let limit = q.limit.unwrap_or(100);
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(limit, q.status.as_deref())
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, StatusCode> {
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .get_task(&id)
        .map(Json)
        .map_err(|_| StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct CreateTaskBody {
    url: String,
    category: Option<String>,
    filename: Option<String>,
    referer: Option<String>,
    cookies: Option<String>,
    force_ytdlp: Option<bool>,
    quality: Option<String>,
    source: Option<String>,
}

async fn create_task(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Json(body): Json<CreateTaskBody>,
) -> Result<(StatusCode, Json<Task>), StatusCode> {
    {
        let limit = state.cfg.read().await.rate_limit_per_minute;
        let key = addr.ip().to_string();
        let mut rl = state.rate_limiter.lock().await;
        rl.limit = limit;
        if !rl.check(&key) {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }

    if body.url.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let input = AddTaskInput {
        url: body.url.trim().to_string(),
        category: body.category.unwrap_or_default(),
        filename: body.filename,
        referer: body.referer,
        cookies: body.cookies,
        force_ytdlp: body.force_ytdlp.unwrap_or(false),
        quality: body.quality,
        source: body.source.or(Some("api".into())),
    };
    state
        .manager
        .add_task(input)
        .await
        .map(|t| (StatusCode::CREATED, Json(t)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn pause_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, StatusCode> {
    state
        .manager
        .pause_task(&id)
        .await
        .map(Json)
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn resume_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, StatusCode> {
    state
        .manager
        .resume_task(&id)
        .await
        .map(Json)
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn remove_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .manager
        .remove_task(&id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn get_settings(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cfg = state.cfg.read().await;
    Json(config::settings_public(&cfg))
}

async fn get_setup(State(state): State<AppState>) -> Json<serde_json::Value> {
    get_settings(State(state)).await
}

#[derive(Deserialize)]
struct SetupBody {
    token: Option<String>,
    default_category: Option<String>,
    ytdlp_path: Option<String>,
    ffmpeg_path: Option<String>,
    ytdlp_quality: Option<String>,
    ytdlp_cookies_file: Option<String>,
    webhook_url: Option<String>,
    webhook_enabled: Option<bool>,
    jellyfin_refresh_url: Option<String>,
    cors_origins: Option<Vec<String>>,
    rate_limit_per_minute: Option<u32>,
    qbit_username: Option<String>,
    qbit_password: Option<String>,
    bt_seed_ratio: Option<f64>,
    bt_seed_time: Option<u32>,
    bt_max_peers: Option<u32>,
    max_upload_kbps: Option<u32>,
    max_download_kbps: Option<u32>,
}

async fn post_setup(
    State(state): State<AppState>,
    Json(body): Json<SetupBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    {
        let mut cfg = state.cfg.write().await;
        apply_settings(&mut cfg, &UpdateSettingsBody {
            default_category: body.default_category,
            ytdlp_path: body.ytdlp_path,
            ffmpeg_path: body.ffmpeg_path,
            ytdlp_quality: body.ytdlp_quality,
            ytdlp_cookies_file: body.ytdlp_cookies_file,
            webhook_url: body.webhook_url,
            webhook_enabled: body.webhook_enabled,
            jellyfin_refresh_url: body.jellyfin_refresh_url,
            cors_origins: body.cors_origins,
            rate_limit_per_minute: body.rate_limit_per_minute,
            qbit_username: body.qbit_username,
            qbit_password: body.qbit_password,
            bt_seed_ratio: body.bt_seed_ratio,
            bt_seed_time: body.bt_seed_time,
            bt_max_peers: body.bt_max_peers,
            max_upload_kbps: body.max_upload_kbps,
            max_download_kbps: body.max_download_kbps,
        });
        if let Some(t) = body.token {
            if !t.is_empty() {
                cfg.token = t;
            }
        }
        cfg.setup_complete = true;
        cfg.save().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    state.manager.version_checker().invalidate().await;
    Ok(get_settings(State(state)).await)
}

#[derive(Deserialize)]
struct UpdateSettingsBody {
    default_category: Option<String>,
    ytdlp_path: Option<String>,
    ffmpeg_path: Option<String>,
    ytdlp_quality: Option<String>,
    ytdlp_cookies_file: Option<String>,
    webhook_url: Option<String>,
    webhook_enabled: Option<bool>,
    jellyfin_refresh_url: Option<String>,
    cors_origins: Option<Vec<String>>,
    rate_limit_per_minute: Option<u32>,
    qbit_username: Option<String>,
    qbit_password: Option<String>,
    bt_seed_ratio: Option<f64>,
    bt_seed_time: Option<u32>,
    bt_max_peers: Option<u32>,
    max_upload_kbps: Option<u32>,
    max_download_kbps: Option<u32>,
}

fn apply_settings(cfg: &mut Config, body: &UpdateSettingsBody) {
    if let Some(c) = &body.default_category {
        cfg.default_category = c.clone();
    }
    if let Some(p) = &body.ytdlp_path {
        cfg.ytdlp_path = p.clone();
    }
    if let Some(p) = &body.ffmpeg_path {
        cfg.ffmpeg_path = p.clone();
    }
    if let Some(v) = &body.ytdlp_quality {
        cfg.ytdlp_quality = v.clone();
    }
    if let Some(v) = &body.ytdlp_cookies_file {
        cfg.ytdlp_cookies_file = Some(v.clone());
    }
    if let Some(v) = &body.webhook_url {
        cfg.webhook_url = Some(v.clone());
    }
    if let Some(v) = body.webhook_enabled {
        cfg.webhook_enabled = v;
    }
    if let Some(v) = &body.jellyfin_refresh_url {
        cfg.jellyfin_refresh_url = Some(v.clone());
    }
    if let Some(v) = &body.cors_origins {
        cfg.cors_origins = v.clone();
    }
    if let Some(v) = body.rate_limit_per_minute {
        cfg.rate_limit_per_minute = v;
    }
    if let Some(v) = &body.qbit_username {
        cfg.qbit_username = v.clone();
    }
    if let Some(v) = &body.qbit_password {
        cfg.qbit_password = v.clone();
    }
    if let Some(v) = body.bt_seed_ratio {
        cfg.bt_seed_ratio = v.max(0.0);
    }
    if let Some(v) = body.bt_seed_time {
        cfg.bt_seed_time = v;
    }
    if let Some(v) = body.bt_max_peers {
        cfg.bt_max_peers = v.clamp(1, 1000);
    }
    if let Some(v) = body.max_upload_kbps {
        cfg.max_upload_kbps = v;
    }
    if let Some(v) = body.max_download_kbps {
        cfg.max_download_kbps = v;
    }
}

async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<UpdateSettingsBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    {
        let mut cfg = state.cfg.write().await;
        apply_settings(&mut cfg, &body);
        cfg.save().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    if let Err(e) = state.manager.apply_bt_settings().await {
        tracing::warn!(err = %e, "failed to apply aria2 bt settings");
    }
    state.manager.version_checker().invalidate().await;
    Ok(get_settings(State(state)).await)
}

#[derive(Serialize)]
struct RegenerateTokenResponse {
    token: String,
}

async fn regenerate_token(
    State(state): State<AppState>,
) -> Result<Json<RegenerateTokenResponse>, StatusCode> {
    let token = {
        let mut cfg = state.cfg.write().await;
        cfg.regenerate_token()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(RegenerateTokenResponse { token }))
}

#[derive(Serialize)]
struct UpdateYtdlpResponse {
    message: String,
}

async fn update_ytdlp(
    State(state): State<AppState>,
) -> Result<Json<UpdateYtdlpResponse>, StatusCode> {
    state
        .manager
        .update_ytdlp()
        .await
        .map(|message| Json(UpdateYtdlpResponse { message }))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct CreateRssFeedBody {
    url: String,
    title: Option<String>,
    filter_regex: Option<String>,
    category: Option<String>,
    enabled: Option<bool>,
    poll_interval_secs: Option<i64>,
}

async fn list_rss_feeds(State(state): State<AppState>) -> Result<Json<Vec<RssFeed>>, StatusCode> {
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_rss_feeds()
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_rss_feed(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RssFeed>, StatusCode> {
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .get_rss_feed(&id)
        .map(Json)
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn create_rss_feed(
    State(state): State<AppState>,
    Json(body): Json<CreateRssFeedBody>,
) -> Result<(StatusCode, Json<RssFeed>), StatusCode> {
    if body.url.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let feed = state
        .manager
        .store()
        .lock()
        .unwrap()
        .create_rss_feed(
            body.url.trim(),
            body.title.as_deref(),
            body.filter_regex.as_deref(),
            &body.category.unwrap_or_default(),
            body.enabled.unwrap_or(true),
            body.poll_interval_secs.unwrap_or(300),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(feed)))
}

#[derive(Deserialize)]
struct UpdateRssFeedBody {
    url: Option<String>,
    title: Option<String>,
    filter_regex: Option<String>,
    category: Option<String>,
    enabled: Option<bool>,
    poll_interval_secs: Option<i64>,
}

async fn update_rss_feed(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRssFeedBody>,
) -> Result<Json<RssFeed>, StatusCode> {
    let mut feed = state
        .manager
        .store()
        .lock()
        .unwrap()
        .get_rss_feed(&id)
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if let Some(v) = body.url {
        feed.url = v;
    }
    if let Some(v) = body.title {
        feed.title = Some(v);
    }
    if let Some(v) = body.filter_regex {
        feed.filter_regex = Some(v);
    }
    if let Some(v) = body.category {
        feed.category = v;
    }
    if let Some(v) = body.enabled {
        feed.enabled = v;
    }
    if let Some(v) = body.poll_interval_secs {
        feed.poll_interval_secs = v;
    }
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .update_rss_feed(&feed)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(feed))
}

async fn delete_rss_feed(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .manager
        .store()
        .lock()
        .unwrap()
        .delete_rss_feed(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::NOT_FOUND)
}
