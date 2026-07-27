use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub addr: String,
    pub token: String,
    pub download_dir: PathBuf,
    pub config_dir: PathBuf,
    pub aria2_rpc_url: String,
    pub aria2_rpc_secret: String,
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    pub default_category: String,
    pub setup_complete: bool,
    pub ytdlp_quality: String,
    pub ytdlp_cookies_file: Option<String>,
    pub webhook_url: Option<String>,
    pub webhook_enabled: bool,
    pub jellyfin_refresh_url: Option<String>,
    pub cors_origins: Vec<String>,
    pub rate_limit_per_minute: u32,
    pub qbit_username: String,
    pub qbit_password: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct FileConfig {
    token: Option<String>,
    ytdlp_path: Option<String>,
    ffmpeg_path: Option<String>,
    default_category: Option<String>,
    setup_complete: Option<bool>,
    ytdlp_quality: Option<String>,
    ytdlp_cookies_file: Option<String>,
    webhook_url: Option<String>,
    webhook_enabled: Option<bool>,
    jellyfin_refresh_url: Option<String>,
    cors_origins: Option<Vec<String>>,
    rate_limit_per_minute: Option<u32>,
    qbit_username: Option<String>,
    qbit_password: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut cfg = Config {
            addr: env_or("DL_SRV_ADDR", "0.0.0.0:35778"),
            token: std::env::var("DL_SRV_TOKEN").unwrap_or_default(),
            download_dir: PathBuf::from(env_or("DOWNLOAD_DIR", "/data/downloads")),
            config_dir: PathBuf::from(env_or("CONFIG_DIR", "/config")),
            aria2_rpc_url: env_or("ARIA2_RPC_URL", "http://127.0.0.1:6800/jsonrpc"),
            aria2_rpc_secret: env_or("ARIA2_RPC_SECRET", "dl-srv-secret"),
            ytdlp_path: env_or("YTDLP_PATH", "yt-dlp"),
            ffmpeg_path: env_or("FFMPEG_PATH", "ffmpeg"),
            default_category: "inbox".into(),
            setup_complete: false,
            ytdlp_quality: "best".into(),
            ytdlp_cookies_file: None,
            webhook_url: None,
            webhook_enabled: false,
            jellyfin_refresh_url: None,
            cors_origins: Vec::new(),
            rate_limit_per_minute: 60,
            qbit_username: "admin".into(),
            qbit_password: "adminadmin".into(),
        };

        fs::create_dir_all(&cfg.config_dir).context("create config dir")?;
        fs::create_dir_all(&cfg.download_dir).context("create download dir")?;

        let path = cfg.config_dir.join("config.json");
        if path.exists() {
            let data = fs::read_to_string(&path)?;
            let fc: FileConfig = serde_json::from_str(&data)?;
            if cfg.token.is_empty() {
                if let Some(t) = fc.token {
                    cfg.token = t;
                }
            }
            if let Some(p) = fc.ytdlp_path {
                cfg.ytdlp_path = p;
            }
            if let Some(p) = fc.ffmpeg_path {
                cfg.ffmpeg_path = p;
            }
            if let Some(c) = fc.default_category {
                cfg.default_category = c;
            }
            if let Some(v) = fc.setup_complete {
                cfg.setup_complete = v;
            }
            if let Some(v) = fc.ytdlp_quality {
                cfg.ytdlp_quality = v;
            }
            cfg.ytdlp_cookies_file = fc.ytdlp_cookies_file;
            cfg.webhook_url = fc.webhook_url;
            if let Some(v) = fc.webhook_enabled {
                cfg.webhook_enabled = v;
            }
            cfg.jellyfin_refresh_url = fc.jellyfin_refresh_url;
            if let Some(v) = fc.cors_origins {
                cfg.cors_origins = v;
            }
            if let Some(v) = fc.rate_limit_per_minute {
                cfg.rate_limit_per_minute = v;
            }
            if let Some(v) = fc.qbit_username {
                cfg.qbit_username = v;
            }
            if let Some(v) = fc.qbit_password {
                cfg.qbit_password = v;
            }
        }

        let custom_ytdlp = cfg.config_dir.join("bin").join("yt-dlp");
        if custom_ytdlp.exists() {
            cfg.ytdlp_path = custom_ytdlp.to_string_lossy().into();
        }

        if cfg.token.is_empty() {
            cfg.token = random_token();
            cfg.save()?;
            tracing::info!(
                "generated new API token — check {}/config.json",
                cfg.config_dir.display()
            );
        }

        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let fc = FileConfig {
            token: Some(self.token.clone()),
            ytdlp_path: Some(self.ytdlp_path.clone()),
            ffmpeg_path: Some(self.ffmpeg_path.clone()),
            default_category: Some(self.default_category.clone()),
            setup_complete: Some(self.setup_complete),
            ytdlp_quality: Some(self.ytdlp_quality.clone()),
            ytdlp_cookies_file: self.ytdlp_cookies_file.clone(),
            webhook_url: self.webhook_url.clone(),
            webhook_enabled: Some(self.webhook_enabled),
            jellyfin_refresh_url: self.jellyfin_refresh_url.clone(),
            cors_origins: Some(self.cors_origins.clone()),
            rate_limit_per_minute: Some(self.rate_limit_per_minute),
            qbit_username: Some(self.qbit_username.clone()),
            qbit_password: Some(self.qbit_password.clone()),
        };
        let data = serde_json::to_string_pretty(&fc)?;
        fs::write(self.config_dir.join("config.json"), data)?;
        Ok(())
    }

    pub fn regenerate_token(&mut self) -> Result<String> {
        self.token = random_token();
        self.save()?;
        Ok(self.token.clone())
    }

    pub fn category_dir(&self, category: &str) -> PathBuf {
        let cat = if category.is_empty() {
            self.default_category.as_str()
        } else {
            category
        };
        self.download_dir.join(cat)
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn settings_public(cfg: &Config) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "download_dir": cfg.download_dir,
        "default_category": cfg.default_category,
        "ytdlp_path": cfg.ytdlp_path,
        "ffmpeg_path": cfg.ffmpeg_path,
        "has_token": !cfg.token.is_empty(),
        "setup_complete": cfg.setup_complete,
        "ytdlp_quality": cfg.ytdlp_quality,
        "ytdlp_cookies_file": cfg.ytdlp_cookies_file,
        "webhook_url": cfg.webhook_url,
        "webhook_enabled": cfg.webhook_enabled,
        "jellyfin_refresh_url": cfg.jellyfin_refresh_url,
        "cors_origins": cfg.cors_origins,
        "rate_limit_per_minute": cfg.rate_limit_per_minute,
        "qbit_username": cfg.qbit_username,
    });
    if !cfg.setup_complete {
        if let Some(map) = obj.as_object_mut() {
            map.insert("token".into(), serde_json::json!(cfg.token));
        }
    }
    obj
}

pub fn ensure_category_dir(cfg: &Config, category: &str) -> Result<PathBuf> {
    let dir = cfg.category_dir(category);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}
