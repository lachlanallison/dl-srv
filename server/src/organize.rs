use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::aria2::{self, Aria2Client};
use crate::config::Config;
use crate::store::{Store, Task, TaskType};

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "m4v", "avi", "webm"];
const INBOX_DIR: &str = "inbox";
const TMDB_BASE: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Movie { title: String, year: u16 },
    Tv { show: String, season: u32, episode: u32 },
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanItem {
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ScanResult {
    pub moved: u32,
    pub skipped: u32,
    pub dry_run: bool,
    pub items: Vec<ScanItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LastResult {
    pub moved: u32,
    pub skipped: u32,
    pub items: Vec<ScanItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryStatus {
    pub enabled: bool,
    pub last_scan_at: Option<String>,
    pub last_result: Option<LastResult>,
}

#[derive(Debug)]
pub enum ScanError {
    Disabled,
    NoTmdbKey,
}

impl ScanError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Disabled => "library organiser is disabled",
            Self::NoTmdbKey => "TMDB API key is not set",
        }
    }
}

#[derive(Debug)]
struct TmdbTitle {
    id: i64,
    title: String,
    year: Option<u16>,
}

pub struct Organizer {
    cfg: Arc<RwLock<Config>>,
    store: Arc<Mutex<Store>>,
    aria2: Aria2Client,
    lock: tokio::sync::Mutex<()>,
    last_scan_at: RwLock<Option<String>>,
    last_result: RwLock<Option<LastResult>>,
    empty_key_logged: AtomicBool,
    http: reqwest::Client,
}

impl Organizer {
    pub fn new(cfg: Arc<RwLock<Config>>, store: Arc<Mutex<Store>>, aria2: Aria2Client) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Arc::new(Self {
            cfg,
            store,
            aria2,
            lock: tokio::sync::Mutex::new(()),
            last_scan_at: RwLock::new(None),
            last_result: RwLock::new(None),
            empty_key_logged: AtomicBool::new(false),
            http,
        })
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                let cfg = self.cfg.read().await.clone();
                if !is_active(&cfg) {
                    self.log_empty_key_once(&cfg);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                if cfg.organize_scan_secs == 0 {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                if let Err(e) = self.scan(false).await {
                    warn!(err = e.message(), "library scan skipped");
                }
                let secs = self.cfg.read().await.organize_scan_secs;
                let wait = if secs == 0 { 60 } else { secs };
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        });
    }

    pub async fn status(&self) -> LibraryStatus {
        let cfg = self.cfg.read().await;
        LibraryStatus {
            enabled: is_active(&cfg),
            last_scan_at: self.last_scan_at.read().await.clone(),
            last_result: self.last_result.read().await.clone(),
        }
    }

    pub async fn scan(&self, full: bool) -> Result<ScanResult, ScanError> {
        let cfg = self.cfg.read().await.clone();
        self.check_active(&cfg)?;
        let _guard = self.lock.lock().await;
        let files = collect_scan_files(&cfg, full);
        let result = self.organize_files(&cfg, &files, None).await;
        self.remember(&result).await;
        maybe_refresh_jellyfin(&self.http, &cfg, &result).await;
        Ok(result)
    }

    pub async fn organize_task(&self, task: &Task) -> ScanResult {
        let cfg = self.cfg.read().await.clone();
        if !is_active(&cfg) {
            self.log_empty_key_once(&cfg);
            return ScanResult {
                dry_run: cfg.organize_dry_run,
                ..ScanResult::default()
            };
        }
        let _guard = self.lock.lock().await;
        let files = self.task_files(task).await;
        self.organize_files(&cfg, &files, Some(task)).await
    }

    fn check_active(&self, cfg: &Config) -> Result<(), ScanError> {
        if !cfg.organize_enabled {
            return Err(ScanError::Disabled);
        }
        if cfg.tmdb_api_key.trim().is_empty() {
            self.log_empty_key_once(cfg);
            return Err(ScanError::NoTmdbKey);
        }
        Ok(())
    }

    fn log_empty_key_once(&self, cfg: &Config) {
        if cfg.organize_enabled && cfg.tmdb_api_key.trim().is_empty() {
            if !self.empty_key_logged.swap(true, Ordering::Relaxed) {
                warn!("library organiser enabled but tmdb_api_key is empty — treating as disabled");
            }
        } else {
            self.empty_key_logged.store(false, Ordering::Relaxed);
        }
    }

    async fn remember(&self, result: &ScanResult) {
        *self.last_scan_at.write().await = Some(Utc::now().to_rfc3339());
        *self.last_result.write().await = Some(LastResult {
            moved: result.moved,
            skipped: result.skipped,
            items: result.items.clone(),
        });
    }

    async fn task_files(&self, task: &Task) -> Vec<PathBuf> {
        if let Some(gid) = &task.backend_gid {
            if let Ok(st) = self.aria2.tell_status(gid).await {
                let files: Vec<PathBuf> = st
                    .files
                    .iter()
                    .map(|f| PathBuf::from(&f.path))
                    .filter(|p| is_video_path(p))
                    .collect();
                if !files.is_empty() {
                    return files;
                }
            }
        }
        fallback_task_file(task).into_iter().collect()
    }

    async fn organize_files(
        &self,
        cfg: &Config,
        files: &[PathBuf],
        task: Option<&Task>,
    ) -> ScanResult {
        let skips = SkipIndex::from_store(&self.store, task);
        let mut result = ScanResult {
            dry_run: cfg.organize_dry_run,
            ..ScanResult::default()
        };
        for path in files {
            let item = self.organize_one(cfg, path, task, &skips).await;
            if item.action == "moved" {
                result.moved += 1;
            } else {
                result.skipped += 1;
            }
            result.items.push(item);
        }
        result
    }

    async fn organize_one(
        &self,
        cfg: &Config,
        path: &Path,
        task: Option<&Task>,
        skips: &SkipIndex,
    ) -> ScanItem {
        let from = path.display().to_string();
        if !path.is_file() {
            return skip_item(from, "missing");
        }
        if !is_video_path(path) {
            return skip_item(from, "not_video");
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.to_ascii_lowercase().contains("sample") {
            return skip_item(from, "sample");
        }
        if has_incomplete_sibling(path) {
            return skip_item(from, "incomplete");
        }
        if let Some(reason) = skips.reason(path) {
            return skip_item(from, reason);
        }
        if task_is_seeding(task, &self.aria2).await {
            return skip_item(from, "seeding");
        }

        let parsed = match parse_filename(&name) {
            Some(p) => p,
            None => return skip_item(from, "unparsed"),
        };

        let dest = match self.resolve_dest(cfg, &parsed, path).await {
            Ok(p) => p,
            Err(reason) => return skip_item(from, reason),
        };

        if same_path(path, &dest) {
            return skip_item(from, "already_organised");
        }
        if dest.exists() {
            return skip_item(from, "exists");
        }

        let to = dest.display().to_string();
        if cfg.organize_dry_run {
            info!(from = %from, to = %to, "library move (dry run)");
            return ScanItem {
                from,
                to: Some(to),
                reason: None,
                action: "moved".into(),
            };
        }

        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!(path = %parent.display(), err = %e, "library mkdir failed");
                return skip_item(from, "mkdir_failed");
            }
        }
        if let Err(e) = std::fs::rename(path, &dest) {
            warn!(from = %from, to = %to, err = %e, "library rename failed");
            return skip_item(from, "rename_failed");
        }
        info!(from = %from, to = %to, "library move");
        move_sidecars(path, &dest);
        ScanItem {
            from,
            to: Some(to),
            reason: None,
            action: "moved".into(),
        }
    }

    async fn resolve_dest(
        &self,
        cfg: &Config,
        parsed: &Parsed,
        src: &Path,
    ) -> Result<PathBuf, &'static str> {
        let ext = src
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        match parsed {
            Parsed::Movie { title, year } => {
                let hit = tmdb_search_movie(&self.http, &cfg.tmdb_api_key, title, *year).await?;
                let dest_title = sanitize_name(&hit.title);
                if dest_title.is_empty() {
                    return Err("no_tmdb_match");
                }
                let dest_year = hit.year.unwrap_or(*year);
                let folder = format!("{dest_title} ({dest_year})");
                Ok(cfg
                    .download_dir
                    .join(&cfg.organize_movies_dir)
                    .join(&folder)
                    .join(format!("{folder}{ext}")))
            }
            Parsed::Tv {
                show,
                season,
                episode,
            } => {
                let hit = tmdb_search_tv(&self.http, &cfg.tmdb_api_key, show).await?;
                let ep_title =
                    tmdb_episode(&self.http, &cfg.tmdb_api_key, hit.id, *season, *episode).await?;
                let show_name = sanitize_name(&hit.title);
                let ep_name = sanitize_name(&ep_title);
                if show_name.is_empty() || ep_name.is_empty() {
                    return Err("no_tmdb_match");
                }
                let year = hit.year.ok_or("no_year")?;
                let show_folder = format!("{show_name} ({year})");
                let season_folder = format!("Season {season:02}");
                let fname = format!("{show_name} - S{season:02}E{episode:02} - {ep_name}{ext}");
                Ok(cfg
                    .download_dir
                    .join(&cfg.organize_tv_dir)
                    .join(show_folder)
                    .join(season_folder)
                    .join(fname))
            }
        }
    }
}

pub fn is_active(cfg: &Config) -> bool {
    cfg.organize_enabled && !cfg.tmdb_api_key.trim().is_empty()
}

pub fn parse_filename(name: &str) -> Option<Parsed> {
    let stem = stem_of(name);
    if stem.is_empty() {
        return None;
    }
    if let Some(caps) = tv_re().captures(&stem) {
        let season: u32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: u32 = caps.get(2)?.as_str().parse().ok()?;
        let prefix = &stem[..caps.get(0)?.start()];
        let show = title_from_prefix(prefix);
        if show.is_empty() {
            return None;
        }
        return Some(Parsed::Tv {
            show,
            season,
            episode,
        });
    }
    if let Some((idx, year)) = find_year(&stem) {
        let title = title_from_prefix(&stem[..idx]);
        if title.is_empty() {
            return None;
        }
        return Some(Parsed::Movie { title, year });
    }
    None
}

pub async fn maybe_refresh_jellyfin(client: &reqwest::Client, cfg: &Config, result: &ScanResult) {
    if result.moved == 0 || result.dry_run {
        return;
    }
    refresh_jellyfin(client, cfg).await;
}

pub async fn refresh_jellyfin(client: &reqwest::Client, cfg: &Config) {
    let Some(url) = &cfg.jellyfin_refresh_url else {
        return;
    };
    if url.trim().is_empty() {
        return;
    }
    match client.get(url).send().await {
        Ok(resp) if !resp.status().is_success() => {
            warn!(status = %resp.status(), url = %url, "jellyfin refresh returned error status");
        }
        Err(e) => warn!(err = %e, url = %url, "jellyfin refresh request failed"),
        _ => {}
    }
}

fn skip_item(from: String, reason: &str) -> ScanItem {
    info!(path = %from, reason, "library skip");
    ScanItem {
        from,
        to: None,
        reason: Some(reason.to_string()),
        action: "skipped".into(),
    }
}

fn stem_of(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

fn title_from_prefix(prefix: &str) -> String {
    let spaced: String = prefix
        .chars()
        .map(|c| if c == '.' || c == '_' { ' ' } else { c })
        .collect();
    spaced
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| c == '-' || c == '.' || c == '_'))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_year(s: &str) -> Option<(usize, u16)> {
    let bytes = s.as_bytes();
    for m in year_re().find_iter(s) {
        let before_ok = m.start() == 0 || !bytes[m.start() - 1].is_ascii_digit();
        let after_ok = m.end() == s.len() || !bytes[m.end()].is_ascii_digit();
        if before_ok && after_ok {
            let year: u16 = m.as_str().parse().ok()?;
            return Some((m.start(), year));
        }
    }
    None
}

fn tv_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)S(\d{1,2})E(\d{1,2})").unwrap())
}

fn year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:19|20)\d{2}").unwrap())
}

fn title_year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^.+ \(\d{4}\)$").unwrap())
}

fn season_dir_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^Season \d{2}$").unwrap())
}

fn tv_dest_file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^.+ - S\d{2}E\d{2} - .+$").unwrap())
}

pub fn normalise_title(s: &str) -> String {
    let replaced = s.to_ascii_lowercase().replace('&', "and");
    replaced
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn sanitize_name(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    mapped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches('.')
        .trim()
        .to_string()
}

fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn has_incomplete_sibling(path: &Path) -> bool {
    let aria2 = path_with_suffix(path, ".aria2");
    let part_suffix = path_with_suffix(path, ".part");
    let part_ext = path.with_extension("part");
    aria2.exists() || part_suffix.exists() || part_ext.exists()
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn looks_organised(path: &Path, cfg: &Config) -> bool {
    looks_organised_movie(path, &cfg.download_dir.join(&cfg.organize_movies_dir))
        || looks_organised_tv(path, &cfg.download_dir.join(&cfg.organize_tv_dir))
}

fn looks_organised_movie(path: &Path, movies_dir: &Path) -> bool {
    let parent = match path.parent() {
        Some(p) => p,
        None => return false,
    };
    if parent.parent() != Some(movies_dir) {
        return false;
    }
    let folder = match parent.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    let stem = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    folder == stem && title_year_re().is_match(folder)
}

fn looks_organised_tv(path: &Path, tv_dir: &Path) -> bool {
    let season_dir = match path.parent() {
        Some(p) => p,
        None => return false,
    };
    let show_dir = match season_dir.parent() {
        Some(p) => p,
        None => return false,
    };
    if show_dir.parent() != Some(tv_dir) {
        return false;
    }
    let season_name = match season_dir.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    let show_name = match show_dir.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    let stem = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    season_dir_re().is_match(season_name)
        && title_year_re().is_match(show_name)
        && tv_dest_file_re().is_match(stem)
}

fn collect_scan_files(cfg: &Config, full: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let roots = [
        cfg.download_dir.join(INBOX_DIR),
        cfg.download_dir.join(&cfg.organize_movies_dir),
        cfg.download_dir.join(&cfg.organize_tv_dir),
    ];
    for root in roots {
        walk_videos(&root, &mut files);
    }
    if !full {
        files.retain(|p| !looks_organised(p, cfg));
    }
    files
}

fn walk_videos(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            walk_videos(&path, out);
        } else if ft.is_file() && is_video_path(&path) {
            out.push(path);
        }
    }
}

fn fallback_task_file(task: &Task) -> Option<PathBuf> {
    let dir = task.save_path.as_ref()?;
    let name = task.filename.as_ref()?;
    Some(PathBuf::from(dir).join(name))
}

fn is_bt_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("magnet:") || lower.contains(".torrent")
}

struct SkipIndex {
    downloading_files: Vec<PathBuf>,
    downloading_prefixes: Vec<PathBuf>,
    seeding_files: Vec<PathBuf>,
    seeding_prefixes: Vec<PathBuf>,
}

impl SkipIndex {
    fn from_store(store: &Mutex<Store>, current: Option<&Task>) -> Self {
        let mut idx = Self {
            downloading_files: Vec::new(),
            downloading_prefixes: Vec::new(),
            seeding_files: Vec::new(),
            seeding_prefixes: Vec::new(),
        };
        let Ok(store) = store.lock() else {
            return idx;
        };
        if let Ok(active) = store.active_tasks() {
            for t in active {
                if current.map(|c| c.id == t.id).unwrap_or(false) {
                    continue;
                }
                add_task_paths(
                    &t,
                    &mut idx.downloading_files,
                    &mut idx.downloading_prefixes,
                );
            }
        }
        if let Ok(done) = store.list_tasks(500, Some("completed")) {
            for t in done {
                if !t.seeding {
                    continue;
                }
                if current.map(|c| c.id == t.id).unwrap_or(false) {
                    continue;
                }
                add_task_paths(&t, &mut idx.seeding_files, &mut idx.seeding_prefixes);
            }
        }
        idx
    }

    fn reason(&self, path: &Path) -> Option<&'static str> {
        if self.downloading_files.iter().any(|p| p == path)
            || self.downloading_prefixes.iter().any(|p| path.starts_with(p))
        {
            return Some("downloading");
        }
        if self.seeding_files.iter().any(|p| p == path)
            || self.seeding_prefixes.iter().any(|p| path.starts_with(p))
        {
            return Some("seeding");
        }
        None
    }
}

fn add_task_paths(task: &Task, files: &mut Vec<PathBuf>, prefixes: &mut Vec<PathBuf>) {
    if let (Some(dir), Some(name)) = (&task.save_path, &task.filename) {
        files.push(PathBuf::from(dir).join(name));
    }
    if is_bt_url(&task.url) {
        if let Some(dir) = &task.save_path {
            prefixes.push(PathBuf::from(dir));
        }
    }
}

async fn task_is_seeding(task: Option<&Task>, aria2: &Aria2Client) -> bool {
    let Some(task) = task else {
        return false;
    };
    if task.seeding {
        return true;
    }
    if task.task_type != TaskType::Aria2 || !is_bt_url(&task.url) {
        return false;
    }
    let Some(gid) = &task.backend_gid else {
        return false;
    };
    match aria2.tell_status(gid).await {
        Ok(st) => aria2::is_seeder(&st),
        Err(_) => false,
    }
}

fn move_sidecars(src: &Path, dest: &Path) {
    let Some(stem) = src.file_stem() else {
        return;
    };
    let Some(parent) = src.parent() else {
        return;
    };
    let Some(dest_stem) = dest.file_stem() else {
        return;
    };
    let Some(dest_parent) = dest.parent() else {
        return;
    };
    for ext in ["srt", "ass"] {
        let from = sibling_with_ext(parent, stem, ext);
        if !from.is_file() {
            continue;
        }
        let to = sibling_with_ext(dest_parent, dest_stem, ext);
        if to.exists() {
            info!(
                from = %from.display(),
                reason = "exists",
                "library sidecar skip"
            );
            continue;
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => info!(from = %from.display(), to = %to.display(), "library sidecar move"),
            Err(e) => warn!(from = %from.display(), err = %e, "library sidecar rename failed"),
        }
    }
}

fn sibling_with_ext(dir: &Path, stem: &std::ffi::OsStr, ext: &str) -> PathBuf {
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(ext);
    dir.join(name)
}

async fn tmdb_search_movie(
    client: &reqwest::Client,
    key: &str,
    query: &str,
    year: u16,
) -> Result<TmdbTitle, &'static str> {
    let url = format!("{TMDB_BASE}/search/movie");
    let resp = client
        .get(&url)
        .query(&[
            ("api_key", key),
            ("query", query),
            ("year", &year.to_string()),
        ])
        .send()
        .await
        .map_err(|_| "tmdb_error")?;
    if !resp.status().is_success() {
        return Err("tmdb_error");
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| "tmdb_error")?;
    pick_tmdb_result(&body, query, Some(year), "title", "release_date")
}

async fn tmdb_search_tv(
    client: &reqwest::Client,
    key: &str,
    query: &str,
) -> Result<TmdbTitle, &'static str> {
    let url = format!("{TMDB_BASE}/search/tv");
    let resp = client
        .get(&url)
        .query(&[("api_key", key), ("query", query)])
        .send()
        .await
        .map_err(|_| "tmdb_error")?;
    if !resp.status().is_success() {
        return Err("tmdb_error");
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| "tmdb_error")?;
    pick_tmdb_result(&body, query, None, "name", "first_air_date")
}

async fn tmdb_episode(
    client: &reqwest::Client,
    key: &str,
    show_id: i64,
    season: u32,
    episode: u32,
) -> Result<String, &'static str> {
    let url = format!("{TMDB_BASE}/tv/{show_id}/season/{season}/episode/{episode}");
    let resp = client
        .get(&url)
        .query(&[("api_key", key)])
        .send()
        .await
        .map_err(|_| "tmdb_error")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("episode_not_found");
    }
    if !resp.status().is_success() {
        return Err("tmdb_error");
    }
    let body: serde_json::Value = resp.json().await.map_err(|_| "tmdb_error")?;
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.trim().is_empty() {
        return Err("episode_not_found");
    }
    Ok(name.to_string())
}

fn pick_tmdb_result(
    body: &serde_json::Value,
    query: &str,
    year: Option<u16>,
    title_key: &str,
    date_key: &str,
) -> Result<TmdbTitle, &'static str> {
    let results = body
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if results.is_empty() {
        return Err("no_tmdb_match");
    }
    let to_hit = |v: &serde_json::Value| -> Option<TmdbTitle> {
        let id = v.get("id")?.as_i64()?;
        let title = v.get(title_key)?.as_str()?.to_string();
        let year = v
            .get(date_key)
            .and_then(|d| d.as_str())
            .and_then(year_from_date);
        Some(TmdbTitle { id, title, year })
    };
    if results.len() == 1 {
        return to_hit(&results[0]).ok_or("no_tmdb_match");
    }
    let first = to_hit(&results[0]).ok_or("ambiguous")?;
    if normalise_title(&first.title) != normalise_title(query) {
        return Err("ambiguous");
    }
    if let Some(y) = year {
        if first.year != Some(y) {
            return Err("ambiguous");
        }
    }
    Ok(first)
}

fn year_from_date(s: &str) -> Option<u16> {
    s.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chef_and_my_fridge() {
        let name =
            "Chef.and.My.Fridge.S02E77.1080p.NF.WEB-DL.x264.AAC2.0-LoveBug [DRAMADAY.me].mkv";
        assert_eq!(
            parse_filename(name),
            Some(Parsed::Tv {
                show: "Chef and My Fridge".into(),
                season: 2,
                episode: 77,
            })
        );
    }

    #[test]
    fn parses_s1e2_and_s01e02() {
        assert_eq!(
            parse_filename("Show.Name.S1E2.mkv"),
            Some(Parsed::Tv {
                show: "Show Name".into(),
                season: 1,
                episode: 2,
            })
        );
        assert_eq!(
            parse_filename("show.name.s01e02.1080p.mkv"),
            Some(Parsed::Tv {
                show: "show name".into(),
                season: 1,
                episode: 2,
            })
        );
    }

    #[test]
    fn parses_movie_with_quality_tags() {
        assert_eq!(
            parse_filename("Movie.Name.2020.1080p.BluRay.x264-GROUP.mkv"),
            Some(Parsed::Movie {
                title: "Movie Name".into(),
                year: 2020,
            })
        );
    }

    #[test]
    fn parses_underscores_and_spaces() {
        assert_eq!(
            parse_filename("The_Matrix_1999.mkv"),
            Some(Parsed::Movie {
                title: "The Matrix".into(),
                year: 1999,
            })
        );
        assert_eq!(
            parse_filename("The Matrix 1999.mp4"),
            Some(Parsed::Movie {
                title: "The Matrix".into(),
                year: 1999,
            })
        );
    }

    #[test]
    fn tv_wins_over_year() {
        assert_eq!(
            parse_filename("Show.Name.S01E01.2020.1080p.mkv"),
            Some(Parsed::Tv {
                show: "Show Name".into(),
                season: 1,
                episode: 1,
            })
        );
    }

    #[test]
    fn skips_unparsed_garbage() {
        assert_eq!(parse_filename("random-file.mkv"), None);
        assert_eq!(parse_filename("VID_1234.mp4"), None);
        assert_eq!(parse_filename("nfo-only.txt"), None);
    }

    #[test]
    fn dash_separated_tv() {
        assert_eq!(
            parse_filename("Show Name - S01E01 - 1080p.mkv"),
            Some(Parsed::Tv {
                show: "Show Name".into(),
                season: 1,
                episode: 1,
            })
        );
    }

    #[test]
    fn normalise_ampersand() {
        assert_eq!(
            normalise_title("Chef & My Fridge"),
            normalise_title("Chef and My Fridge")
        );
        assert_eq!(
            normalise_title("Chef.and.My.Fridge"),
            "chef and my fridge"
        );
    }

    #[test]
    fn sanitise_windows_chars() {
        assert_eq!(sanitize_name("Star Wars: Episode IV"), "Star Wars_ Episode IV");
        assert_eq!(sanitize_name("What? / A *Title*"), "What_ _ A _Title_");
    }

    #[test]
    fn looks_organised_movie_path() {
        let movies = PathBuf::from("/media/movies");
        let organised = movies.join("Foo (2020)").join("Foo (2020).mkv");
        let flat = movies.join("Foo.2020.1080p.mkv");
        assert!(looks_organised_movie(&organised, &movies));
        assert!(!looks_organised_movie(&flat, &movies));
    }

    #[test]
    fn looks_organised_tv_path() {
        let tv = PathBuf::from("/media/tv");
        let organised = tv
            .join("Show (2019)")
            .join("Season 01")
            .join("Show - S01E01 - Pilot.mkv");
        let loose = tv.join("Show.S01E01.mkv");
        assert!(looks_organised_tv(&organised, &tv));
        assert!(!looks_organised_tv(&loose, &tv));
    }

    #[test]
    fn pick_single_tmdb_result() {
        let body = serde_json::json!({
            "results": [{ "id": 1, "title": "Other Name", "release_date": "2020-01-01" }]
        });
        let hit = pick_tmdb_result(&body, "Query", Some(2020), "title", "release_date").unwrap();
        assert_eq!(hit.title, "Other Name");
    }

    #[test]
    fn pick_first_when_title_and_year_match() {
        let body = serde_json::json!({
            "results": [
                { "id": 1, "title": "Movie Name", "release_date": "2020-05-01" },
                { "id": 2, "title": "Movie Name 2", "release_date": "2020-01-01" }
            ]
        });
        let hit = pick_tmdb_result(&body, "Movie Name", Some(2020), "title", "release_date").unwrap();
        assert_eq!(hit.id, 1);
    }

    #[test]
    fn skip_ambiguous_tmdb() {
        let body = serde_json::json!({
            "results": [
                { "id": 1, "title": "Foo", "release_date": "2020-01-01" },
                { "id": 2, "title": "Bar", "release_date": "2020-01-01" }
            ]
        });
        let err = pick_tmdb_result(&body, "Query", Some(2020), "title", "release_date").unwrap_err();
        assert_eq!(err, "ambiguous");
    }
}
