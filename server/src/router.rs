use crate::store::TaskType;
use crate::ytdlp::YtdlpRunner;

const VIDEO_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "m.youtube.com",
    "bilibili.com",
    "b23.tv",
    "tiktok.com",
    "douyin.com",
    "twitter.com",
    "x.com",
    "instagram.com",
    "facebook.com",
    "twitch.tv",
    "vimeo.com",
    "dailymotion.com",
    "nicovideo.jp",
    "reddit.com",
    "old.reddit.com",
];

pub fn classify_sync(url: &str, force_ytdlp: bool) -> TaskType {
    if force_ytdlp {
        return TaskType::Ytdlp;
    }
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("magnet:") || is_torrent_url(&lower) {
        return TaskType::Aria2;
    }
    if skip_ytdlp_simulate(&lower) {
        return TaskType::Aria2;
    }
    if is_direct_file_url(&lower) {
        return TaskType::Aria2;
    }
    for host in VIDEO_HOSTS {
        if lower.contains(host) {
            return TaskType::Ytdlp;
        }
    }
    TaskType::Aria2
}

pub async fn classify(url: &str, force_ytdlp: bool, runner: Option<&YtdlpRunner>) -> TaskType {
    let sync = classify_sync(url, force_ytdlp);
    if sync != TaskType::Aria2 {
        return sync;
    }
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("magnet:") || is_torrent_url(&lower) {
        return TaskType::Aria2;
    }
    if skip_ytdlp_simulate(&lower) || is_direct_file_url(&lower) {
        return TaskType::Aria2;
    }
    if let Some(r) = runner {
        if r.simulate(url).await.unwrap_or(false) {
            return TaskType::Ytdlp;
        }
    }
    TaskType::Aria2
}

/// Direct file hosts — always aria2, never yt-dlp simulate probe.
pub fn skip_ytdlp_simulate(lower: &str) -> bool {
    if lower.contains("pixeldrain.com") {
        return lower.contains("/api/file/") || lower.contains("/u/");
    }
    false
}

pub fn is_direct_file_url(lower: &str) -> bool {
    const EXTS: &[&str] = &[
        ".mkv", ".mp4", ".avi", ".mov", ".wmv", ".flv", ".webm", ".m4v", ".ts",
        ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz",
        ".pdf", ".epub", ".mobi",
        ".mp3", ".flac", ".wav", ".aac", ".ogg",
        ".iso", ".img",
    ];
    let path = lower.split('?').next().unwrap_or(lower);
    EXTS.iter().any(|ext| path.ends_with(ext))
}

fn is_torrent_url(lower: &str) -> bool {
    lower.ends_with(".torrent")
        || lower.contains(".torrent?")
        || lower.contains(".torrent&")
}

/// Cheap checks before queueing — rejects obvious junk without spawning yt-dlp/aria2.
pub fn validate_task_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL is empty".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("data:") || lower.starts_with("blob:") || lower.starts_with("javascript:") {
        return Err("Unsupported URL scheme".into());
    }
    if lower.starts_with("magnet:") {
        if !lower.contains("xt=urn:btih:") {
            return Err("Magnet links must include a hash (magnet:?xt=urn:btih:...)".into());
        }
        return Ok(());
    }

    let parsed = url::Url::parse(trimmed).map_err(|_| "Invalid URL".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("URL must be http, https, or magnet".into()),
    }

    if is_direct_file_url(&lower) || is_torrent_url(&lower) || skip_ytdlp_simulate(&lower) {
        return Ok(());
    }
    for host in VIDEO_HOSTS {
        if lower.contains(host) {
            return Ok(());
        }
    }
    if looks_like_search_page(&parsed) {
        return Err(
            "URL looks like a search page — paste a direct magnet, .torrent, file, or video link"
                .into(),
        );
    }

    Ok(())
}

fn looks_like_search_page(parsed: &url::Url) -> bool {
    let path = parsed.path().to_ascii_lowercase();
    if path.contains("/search") {
        return true;
    }
    let Some(q) = parsed.query() else {
        return false;
    };
    let q = q.to_ascii_lowercase();
    q.contains("query=") || q.starts_with("q=")
}
