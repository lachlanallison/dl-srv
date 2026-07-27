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
    for host in VIDEO_HOSTS {
        if lower.contains(host) {
            return TaskType::Ytdlp;
        }
    }
    TaskType::Aria2
}

pub async fn classify(url: &str, force_ytdlp: bool, runner: Option<&YtdlpRunner>) -> TaskType {
    if force_ytdlp {
        return TaskType::Ytdlp;
    }
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("magnet:") || is_torrent_url(&lower) {
        return TaskType::Aria2;
    }
    for host in VIDEO_HOSTS {
        if lower.contains(host) {
            return TaskType::Ytdlp;
        }
    }
    if let Some(r) = runner {
        if r.simulate(url).await.unwrap_or(false) {
            return TaskType::Ytdlp;
        }
    }
    TaskType::Aria2
}

fn is_torrent_url(lower: &str) -> bool {
    lower.ends_with(".torrent")
        || lower.contains(".torrent?")
        || lower.contains(".torrent&")
}
