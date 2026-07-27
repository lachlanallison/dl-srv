mod api;
mod aria2;
mod config;
mod filename;
mod hooks;
mod manager;
mod qbit;
mod router;
mod rss;
mod store;
mod version;
mod ytdlp;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::api::{AppState, RateLimiter};
use crate::aria2::Aria2Client;
use crate::config::Config;
use crate::manager::Manager;
use crate::qbit::QbitState;
use crate::rss::RssPoller;
use crate::store::Store;
use crate::version::VersionChecker;

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("dlsrv=info".parse()?))
        .init();

    let cfg = Config::load()?;
    let listen_addr = cfg.addr.clone();
    let cors_origins = cfg.cors_origins.clone();
    let rate_limit = cfg.rate_limit_per_minute;
    tracing::info!(addr = %listen_addr, download_dir = %cfg.download_dir.display(), "starting dl-srv");

    let db_path = cfg.config_dir.join("tasks.db");
    let store = Store::open(&db_path).context("open sqlite")?;

    let aria2 = Aria2Client::new(cfg.aria2_rpc_url.clone(), cfg.aria2_rpc_secret.clone());
    wait_for_aria2(&aria2).await.context("aria2 not reachable — is aria2c running?")?;

    let (events, _) = broadcast::channel(256);
    let version_checker = Arc::new(VersionChecker::new(
        cfg.ytdlp_path.clone(),
        cfg.ffmpeg_path.clone(),
        aria2.clone(),
    ));

    let cfg = Arc::new(RwLock::new(cfg));
    let manager = Manager::new(
        cfg.clone(),
        store,
        aria2,
        events.clone(),
        version_checker,
    );
    manager.start_background_tasks();

    let store_arc = manager.store();
    let rss_poller = Arc::new(RssPoller::new(manager.clone(), store_arc));
    rss_poller.start();

    let app_state = AppState {
        cfg: cfg.clone(),
        manager: manager.clone(),
        events,
        rate_limiter: Arc::new(tokio::sync::Mutex::new(RateLimiter::new(rate_limit))),
    };

    let qbit_state = QbitState {
        cfg: cfg.clone(),
        manager,
        sessions: Arc::new(RwLock::new(HashSet::new())),
    };

    let web_root = web_dist_dir();
    let api = api::routes(app_state);
    let qbit_api = qbit::routes(qbit_state);

    let app = if web_root.join("index.html").exists() {
        Router::new()
            .nest("/api/v1", api)
            .nest("/api/v2", qbit_api)
            .fallback_service(
                ServeDir::new(&web_root)
                    .not_found_service(ServeFile::new(web_root.join("index.html"))),
            )
    } else {
        tracing::warn!(path = %web_root.display(), "web dist not found — API only");
        Router::new()
            .nest("/api/v1", api)
            .nest("/api/v2", qbit_api)
    };

    let cors = build_cors_layer(&cors_origins);

    let app = app.layer(TraceLayer::new_for_http()).layer(cors);

    let addr: SocketAddr = listen_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn build_cors_layer(origins: &[String]) -> CorsLayer {
    if origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        use axum::http::{HeaderValue, Method};
        let allowed: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(allowed)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(Any)
    }
}

fn web_dist_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WEB_DIST") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist")
}

async fn wait_for_aria2(aria2: &Aria2Client) -> anyhow::Result<()> {
    for attempt in 0..100 {
        if aria2.ping().await.is_ok() {
            return Ok(());
        }
        if attempt == 99 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    anyhow::bail!("aria2 not reachable after 20s")
}
