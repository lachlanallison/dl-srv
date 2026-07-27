use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Form, Json, Router,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::manager::{qbit_category, task_info_hash, Manager};
use crate::store::{AddTaskInput, Task, TaskStatus};

#[derive(Clone)]
pub struct QbitState {
    pub cfg: Arc<RwLock<Config>>,
    pub manager: Arc<Manager>,
    pub sessions: Arc<RwLock<HashSet<String>>>,
}

pub fn routes(state: QbitState) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/app/version", get(app_version))
        .route("/app/webapiVersion", get(webapi_version))
        .route("/app/preferences", get(preferences))
        .route("/torrents/info", get(torrents_info))
        .route("/torrents/properties", get(torrents_properties))
        .route("/torrents/categories", get(torrents_categories))
        .route("/torrents/add", post(torrents_add))
        .route("/torrents/pause", post(torrents_pause))
        .route("/torrents/resume", post(torrents_resume))
        .route("/torrents/delete", post(torrents_delete))
        .route("/transfer/info", get(transfer_info))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            qbit_auth_middleware,
        ))
        .with_state(state)
}

async fn qbit_auth_middleware(
    State(state): State<QbitState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();
    if path.ends_with("/auth/login") {
        return Ok(next.run(request).await);
    }

    let sid = cookie_sid(&headers);
    let sessions = state.sessions.read().await;
    if sid.map(|s| sessions.contains(s)).unwrap_or(false) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn cookie_sid(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix("SID=").map(str::trim)
            })
        })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login(
    State(state): State<QbitState>,
    Form(form): Form<LoginForm>,
) -> Result<(StatusCode, HeaderMap, &'static str), StatusCode> {
    let cfg = state.cfg.read().await;
    if form.username != cfg.qbit_username || form.password != cfg.qbit_password {
        return Err(StatusCode::FORBIDDEN);
    }
    drop(cfg);

    let mut sid_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut sid_bytes);
    let sid = hex::encode(sid_bytes);
    state.sessions.write().await.insert(sid.clone());

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        format!("SID={sid}; HttpOnly; Path=/api/v2")
            .parse()
            .unwrap(),
    );
    Ok((StatusCode::OK, headers, "Ok."))
}

async fn app_version() -> &'static str {
    "v4.6.0"
}

async fn webapi_version() -> &'static str {
    "2.11.0"
}

async fn preferences() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "locale": "en",
        "create_subfolder_enabled": false,
        "start_paused_enabled": false,
        "auto_delete_mode": 0,
    }))
}

#[derive(Debug, Serialize)]
struct QbitTorrent {
    hash: String,
    name: String,
    size: i64,
    progress: f64,
    dlspeed: i64,
    upspeed: i64,
    downloaded: i64,
    uploaded: i64,
    eta: i64,
    state: String,
    category: String,
    tags: String,
    added_on: i64,
    completion_on: i64,
    save_path: String,
    content_path: String,
}

fn map_task_to_qbit(task: &Task) -> QbitTorrent {
    let hash = task_info_hash(task);
    let name = task
        .filename
        .clone()
        .unwrap_or_else(|| task.url.clone());
    let progress = task.progress / 100.0;
    let state = match task.status {
        TaskStatus::Pending => "queuedDL",
        TaskStatus::Downloading => "downloading",
        TaskStatus::Paused => "pausedDL",
        TaskStatus::Completed => "uploading",
        TaskStatus::Failed => "error",
        TaskStatus::Removed => "missingFiles",
    };
    let save_path = task.save_path.clone().unwrap_or_default();
    let completion_on = task
        .completed_at
        .map(|t| t.timestamp())
        .unwrap_or(0);
    QbitTorrent {
        hash,
        name,
        size: task.total_bytes.max(task.done_bytes),
        progress,
        dlspeed: task.speed,
        upspeed: 0,
        downloaded: task.done_bytes,
        uploaded: 0,
        eta: 8640000,
        state: state.into(),
        category: qbit_category(task),
        tags: String::new(),
        added_on: task.created_at.timestamp(),
        completion_on,
        save_path: save_path.clone(),
        content_path: save_path,
    }
}

#[derive(Deserialize)]
struct HashesQuery {
    hashes: Option<String>,
}

async fn torrents_info(
    State(state): State<QbitState>,
    Query(q): Query<HashesQuery>,
) -> Result<Json<Vec<QbitTorrent>>, StatusCode> {
    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let filter: Option<Vec<String>> = q.hashes.map(|h| {
        h.split('|')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    });

    let out: Vec<QbitTorrent> = tasks
        .iter()
        .filter(|t| {
            if let Some(ref hashes) = filter {
                hashes.contains(&task_info_hash(t))
            } else {
                true
            }
        })
        .map(map_task_to_qbit)
        .collect();
    Ok(Json(out))
}

async fn torrents_properties(
    State(state): State<QbitState>,
    Query(q): Query<HashesQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let hash = q
        .hashes
        .as_deref()
        .and_then(|h| h.split('|').next())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let task = tasks
        .iter()
        .find(|t| task_info_hash(t) == hash)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "save_path": task.save_path,
        "creation_date": task.created_at.timestamp(),
        "piece_size": 0,
        "comment": task.url,
        "total_wasted": 0,
        "total_uploaded": 0,
        "total_uploaded_session": 0,
        "total_downloaded": task.done_bytes,
        "total_downloaded_session": task.done_bytes,
        "up_limit": -1,
        "dl_limit": -1,
        "time_elapsed": 0,
        "nb_connections": 0,
        "nb_connections_limit": 100,
        "share_ratio": 0.0,
        "addition_date": task.created_at.timestamp(),
        "completion_date": task.completed_at.map(|t| t.timestamp()).unwrap_or(0),
        "created_by": "dl-srv",
        "dl_speed_avg": task.speed,
        "dl_speed": task.speed,
        "eta": 8640000,
        "last_seen": task.updated_at.timestamp(),
        "peers": 0,
        "peers_total": 0,
        "pieces_have": 0,
        "pieces_num": 0,
        "reannounce": 0,
        "seeds": 0,
        "seeds_total": 0,
        "total_size": task.total_bytes,
    })))
}

async fn torrents_categories(
    State(state): State<QbitState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut cats = serde_json::Map::new();
    for task in &tasks {
        let cat = qbit_category(task);
        cats.insert(cat.clone(), serde_json::json!({ "name": cat, "savePath": task.save_path }));
    }
    Ok(Json(serde_json::Value::Object(cats)))
}

#[derive(Deserialize)]
struct TorrentAddForm {
    urls: Option<String>,
    category: Option<String>,
    paused: Option<String>,
    #[allow(dead_code)]
    savepath: Option<String>,
}

async fn torrents_add(
    State(state): State<QbitState>,
    Form(form): Form<TorrentAddForm>,
) -> Result<&'static str, StatusCode> {
    let urls = form.urls.ok_or(StatusCode::BAD_REQUEST)?;
    let category = form.category.unwrap_or_default();
    let paused = form
        .paused
        .as_deref()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    for url in urls.lines().flat_map(|l| l.split('\n')).filter(|l| !l.trim().is_empty()) {
        let input = AddTaskInput {
            url: url.trim().to_string(),
            category: category.clone(),
            referer: None,
            cookies: None,
            force_ytdlp: false,
            quality: None,
            source: Some("qbit".into()),
        };
        let task = state
            .manager
            .add_task(input)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if paused {
            let _ = state.manager.pause_task(&task.id).await;
        }
    }
    Ok("Ok.")
}

#[derive(Deserialize)]
struct HashesForm {
    hashes: String,
}

async fn torrents_pause(
    State(state): State<QbitState>,
    Form(form): Form<HashesForm>,
) -> Result<&'static str, StatusCode> {
    pause_or_resume(&state, &form.hashes, true).await
}

async fn torrents_resume(
    State(state): State<QbitState>,
    Form(form): Form<HashesForm>,
) -> Result<&'static str, StatusCode> {
    pause_or_resume(&state, &form.hashes, false).await
}

async fn pause_or_resume(state: &QbitState, hashes: &str, pause: bool) -> Result<&'static str, StatusCode> {
    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    for hash in hashes.split('|').filter(|h| !h.is_empty()) {
        if let Some(task) = tasks.iter().find(|t| task_info_hash(t) == hash) {
            if pause {
                let _ = state.manager.pause_task(&task.id).await;
            } else {
                let _ = state.manager.resume_task(&task.id).await;
            }
        }
    }
    Ok("Ok.")
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct DeleteForm {
    hashes: String,
    #[serde(rename = "deleteFiles")]
    _deleteFiles: Option<String>,
}

async fn torrents_delete(
    State(state): State<QbitState>,
    Form(form): Form<DeleteForm>,
) -> Result<&'static str, StatusCode> {
    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    for hash in form.hashes.split('|').filter(|h| !h.is_empty()) {
        if let Some(task) = tasks.iter().find(|t| task_info_hash(t) == hash) {
            let _ = state.manager.remove_task(&task.id).await;
        }
    }
    Ok("Ok.")
}

async fn transfer_info(
    State(state): State<QbitState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let tasks = state
        .manager
        .store()
        .lock()
        .unwrap()
        .list_tasks(1000, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let dl_speed: i64 = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Downloading)
        .map(|t| t.speed)
        .sum();
    let dl_total: i64 = tasks.iter().map(|t| t.done_bytes).sum();

    Ok(Json(serde_json::json!({
        "connection_status": "connected",
        "dht_nodes": 0,
        "dl_info_data": dl_total,
        "dl_info_speed": dl_speed,
        "dl_rate_limit": 0,
        "up_info_data": 0,
        "up_info_speed": 0,
        "up_rate_limit": 0,
    })))
}
