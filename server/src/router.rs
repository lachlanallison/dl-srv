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
    if is_direct_file_url(&lower) {
        return TaskType::Aria2;
    }
    if let Some(r) = runner {
        if r.simulate(url).await.unwrap_or(false) {
            return TaskType::Ytdlp;
        }
    }
    TaskType::Aria2
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
