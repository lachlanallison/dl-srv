use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::aria2::Aria2Client;
use crate::ytdlp::{ffmpeg_version, YtdlpRunner};

#[derive(Debug, Clone, Serialize)]
pub struct BinaryInfo {
    pub name: String,
    pub installed: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    pub update_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub dlsrv_version: String,
    pub aria2_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aria2_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aria2_latest: Option<String>,
    pub aria2_update_available: bool,
    pub binaries: Vec<BinaryInfo>,
    pub checked_at: chrono::DateTime<Utc>,
}

pub struct VersionChecker {
    ytdlp_path: String,
    ffmpeg_path: String,
    aria2: Aria2Client,
    cache: Mutex<Option<(Instant, HealthReport)>>,
}

impl VersionChecker {
    pub fn new(ytdlp_path: String, ffmpeg_path: String, aria2: Aria2Client) -> Self {
        Self {
            ytdlp_path,
            ffmpeg_path,
            aria2,
            cache: Mutex::new(None),
        }
    }

    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    pub async fn health(&self) -> HealthReport {
        if let Some((at, report)) = self.cache.lock().await.clone() {
            if at.elapsed() < Duration::from_secs(900) {
                return report;
            }
        }

        let mut report = HealthReport {
            dlsrv_version: env!("CARGO_PKG_VERSION").to_string(),
            aria2_ok: false,
            aria2_version: None,
            aria2_latest: None,
            aria2_update_available: false,
            binaries: vec![],
            checked_at: Utc::now(),
        };

        let aria2_latest = fetch_github_latest("aria2", "aria2").await.ok();

        match self.aria2.ping().await {
            Ok(()) => {
                report.aria2_ok = true;
                report.aria2_version = self.aria2.version().await.ok();
                if let Some(ref installed) = report.aria2_version {
                    report.aria2_latest = aria2_latest.clone();
                    report.aria2_update_available = aria2_latest
                        .as_ref()
                        .map(|l| aria2_outdated(installed, l))
                        .unwrap_or(false);
                }
            }
            Err(e) => tracing::warn!(err = %e, "aria2 health check failed"),
        }

        let ytdlp_latest = fetch_github_latest("yt-dlp", "yt-dlp").await.ok();
        report.binaries.push(check_ytdlp(&self.ytdlp_path, ytdlp_latest.clone()).await);

        let ffmpeg_latest = fetch_github_latest("FFmpeg", "FFmpeg").await.ok();
        report
            .binaries
            .push(check_ffmpeg(&self.ffmpeg_path, ffmpeg_latest).await);

        *self.cache.lock().await = Some((Instant::now(), report.clone()));
        report
    }
}

async fn fetch_github_latest(owner: &str, repo: &str) -> Result<String, reqwest::Error> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?;
    let resp = client
        .get(url)
        .header("User-Agent", "dl-srv")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;
    #[derive(serde::Deserialize)]
    struct Body {
        tag_name: String,
    }
    Ok(resp.json::<Body>().await?.tag_name)
}

async fn check_ytdlp(path: &str, latest: Option<String>) -> BinaryInfo {
    let runner = YtdlpRunner {
        binary: path.to_string(),
        ffmpeg_path: String::new(),
        aria2_secret: String::new(),
        use_aria2: false,
        quality: "best".into(),
        cookies_file: None,
    };
    match runner.version().await {
        Ok(installed) => {
            let update = latest
                .as_ref()
                .map(|l| ytdlp_outdated(&installed, l))
                .unwrap_or(false);
            BinaryInfo {
                name: "yt-dlp".into(),
                installed,
                latest,
                update_available: update,
                error: None,
            }
        }
        Err(e) => BinaryInfo {
            name: "yt-dlp".into(),
            installed: String::new(),
            latest,
            update_available: false,
            error: Some(e.to_string()),
        },
    }
}

async fn check_ffmpeg(path: &str, latest: Option<String>) -> BinaryInfo {
    match ffmpeg_version(path).await {
        Ok(installed) => {
            let update = latest
                .as_ref()
                .map(|l| ffmpeg_outdated(&installed, l))
                .unwrap_or(false);
            BinaryInfo {
                name: "ffmpeg".into(),
                installed,
                latest,
                update_available: update,
                error: None,
            }
        }
        Err(e) => BinaryInfo {
            name: "ffmpeg".into(),
            installed: String::new(),
            latest,
            update_available: false,
            error: Some(e.to_string()),
        },
    }
}

fn ytdlp_outdated(installed: &str, latest: &str) -> bool {
    let re = regex::Regex::new(r"(\d{4}\.\d{2}\.\d{2})").unwrap();
    let i = re.find(installed).map(|m| m.as_str());
    let l = re.find(latest).map(|m| m.as_str());
    match (i, l) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

fn ffmpeg_outdated(installed: &str, latest: &str) -> bool {
    let ire = regex::Regex::new(r"ffmpeg version ([\d.]+)").unwrap();
    let lre = regex::Regex::new(r"n?(\d+\.\d+(?:\.\d+)?)").unwrap();
    let i = ire.captures(installed).and_then(|c| c.get(1)).map(|m| m.as_str());
    let l = lre.captures(latest).and_then(|c| c.get(1)).map(|m| m.as_str());
    match (i, l) {
        (Some(a), Some(b)) => ver_key(a) < ver_key(b),
        _ => false,
    }
}

fn aria2_outdated(installed: &str, latest: &str) -> bool {
    let latest_ver = latest
        .strip_prefix("release-")
        .unwrap_or(latest)
        .trim_start_matches('n');
    let installed_ver = installed.split_whitespace().next().unwrap_or(installed);
    ver_key(installed_ver) < ver_key(latest_ver)
}

fn ver_key(v: &str) -> String {
    let parts: Vec<_> = v.split('.').collect();
    format!(
        "{:03}{:03}{:03}",
        parts.first().unwrap_or(&"0").parse::<u32>().unwrap_or(0),
        parts.get(1).unwrap_or(&"0").parse::<u32>().unwrap_or(0),
        parts.get(2).unwrap_or(&"0").parse::<u32>().unwrap_or(0),
    )
}
