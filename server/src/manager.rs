use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

use crate::aria2::{self, AddOptions, Aria2Client};
use crate::config::{self, Config};
use crate::hooks;
use crate::router;
use crate::store::{AddTaskInput, Store, Task, TaskStatus, TaskType};
use crate::version::VersionChecker;
use crate::ytdlp::{DownloadOpts, YtdlpJobRegistry, YtdlpRunner};

pub fn task_info_hash(task: &Task) -> String {
    let mut hasher = Sha256::new();
    hasher.update(task.id.as_bytes());
    hex::encode(&hasher.finalize()[..20])
}

pub fn qbit_category(task: &Task) -> String {
    if task.category.is_empty() {
        "inbox".into()
    } else {
        task.category.clone()
    }
}

pub struct Manager {
    cfg: Arc<RwLock<Config>>,
    store: Arc<std::sync::Mutex<Store>>,
    aria2: Aria2Client,
    events: broadcast::Sender<Task>,
    version_checker: Arc<VersionChecker>,
    ytdlp_jobs: Arc<YtdlpJobRegistry>,
    task_cookies: Arc<std::sync::Mutex<HashMap<String, String>>>,
}

impl Manager {
    pub fn new(
        cfg: Arc<RwLock<Config>>,
        store: Store,
        aria2: Aria2Client,
        events: broadcast::Sender<Task>,
        version_checker: Arc<VersionChecker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            store: Arc::new(std::sync::Mutex::new(store)),
            aria2,
            events,
            version_checker,
            ytdlp_jobs: Arc::new(YtdlpJobRegistry::new()),
            task_cookies: Arc::new(std::sync::Mutex::new(HashMap::new())),
        })
    }

    fn emit(&self, task: &Task) {
        let _ = self.events.send(task.clone());
    }

    fn save_and_emit(&self, task: &Task) -> Result<()> {
        self.store.lock().unwrap().update_task(task)?;
        self.emit(task);
        Ok(())
    }

    async fn ytdlp_runner(&self) -> YtdlpRunner {
        let cfg = self.cfg.read().await;
        YtdlpRunner {
            binary: cfg.ytdlp_path.clone(),
            ffmpeg_path: cfg.ffmpeg_path.clone(),
            aria2_secret: cfg.aria2_rpc_secret.clone(),
            use_aria2: true,
            quality: cfg.ytdlp_quality.clone(),
            cookies_file: cfg.ytdlp_cookies_file.clone(),
        }
    }

    pub fn start_background_tasks(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this.apply_bt_settings().await {
                warn!(err = %e, "aria2 bt settings apply failed");
            }
        });

        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tick.tick().await;
                if let Err(e) = this.poll_aria2().await {
                    warn!(err = %e, "aria2 poll error");
                }
            }
        });
    }

    pub fn store(&self) -> Arc<std::sync::Mutex<Store>> {
        self.store.clone()
    }

    pub fn version_checker(&self) -> Arc<VersionChecker> {
        self.version_checker.clone()
    }

    pub async fn apply_bt_settings(&self) -> Result<()> {
        let cfg = self.cfg.read().await;
        self.aria2
            .change_global_bt_options(&cfg.bt_settings())
            .await
    }

    pub async fn add_task(self: &Arc<Self>, input: AddTaskInput) -> Result<Task> {
        let cfg = self.cfg.read().await;
        let category = if input.category.is_empty() {
            cfg.default_category.clone()
        } else {
            input.category.clone()
        };
        let save_dir = config::ensure_category_dir(&cfg, &category)?;
        let save_path = save_dir.to_string_lossy().into_owned();
        drop(cfg);

        let task_type = router::classify_sync(&input.url, input.force_ytdlp);
        let task = self
            .store
            .lock()
            .unwrap()
            .create_task(&input, task_type, &save_path)?;
        self.emit(&task);

        info!(
            task_id = %task.id,
            category = %task.category,
            backend = %task.task_type.as_str(),
            force_ytdlp = input.force_ytdlp,
            source = ?input.source,
            url = %log_url(&input.url),
            "download queued"
        );

        let task_id = task.id.clone();
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this.start_task(task_id.clone(), input).await {
                error!(task_id = %task_id, err = %e, "start task failed");
                if let Ok(mut t) = this.store.lock().unwrap().get_task(&task_id) {
                    t.status = TaskStatus::Failed;
                    t.error = Some(e.to_string());
                    t.updated_at = Utc::now();
                    let _ = this.save_and_emit(&t);
                }
            }
        });

        Ok(task)
    }

    async fn start_task(self: &Arc<Self>, task_id: String, input: AddTaskInput) -> Result<()> {
        let runner = self.ytdlp_runner().await;
        let cookies = input
            .cookies
            .as_ref()
            .filter(|c| !c.trim().is_empty())
            .map(|c| c.trim().to_string());

        let lower = input.url.trim().to_ascii_lowercase();
        let mut task_type = router::classify_sync(&input.url, input.force_ytdlp);
        let is_bt = lower.starts_with("magnet:") || is_torrent_url(&lower);
        if task_type == TaskType::Aria2
            && !router::skip_ytdlp_simulate(&lower)
            && !router::is_direct_file_url(&lower)
            && !is_bt
        {
            task_type = match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                router::classify(&input.url, input.force_ytdlp, Some(&runner)),
            )
            .await
            {
                Ok(t) => t,
                Err(_) => TaskType::Aria2,
            };
        }

        let mut task = self.store.lock().unwrap().get_task(&task_id)?;
        if task.task_type != task_type {
            task.task_type = task_type;
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;
        }

        if let Some(ref c) = cookies {
            if task_type == TaskType::Ytdlp {
                self.task_cookies
                    .lock()
                    .unwrap()
                    .insert(task_id.clone(), c.clone());
            }
        }

        match task_type {
            TaskType::Aria2 => {
                if let Err(e) = self.start_aria2_task(&mut task, cookies.clone()).await {
                    if !router::skip_ytdlp_simulate(&lower)
                        && !router::is_direct_file_url(&lower)
                        && runner.simulate(&input.url).await.unwrap_or(false)
                    {
                        if let Some(c) = cookies {
                            self.task_cookies.lock().unwrap().insert(task_id, c);
                        }
                        task.task_type = TaskType::Ytdlp;
                        task.updated_at = Utc::now();
                        self.save_and_emit(&task)?;
                        self.spawn_ytdlp(task.id.clone());
                    } else {
                        return Err(e);
                    }
                }
            }
            TaskType::Ytdlp => {
                self.save_and_emit(&task)?;
                self.spawn_ytdlp(task.id.clone());
            }
        }

        Ok(())
    }

    fn spawn_ytdlp(self: &Arc<Self>, task_id: String) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this.run_ytdlp(&task_id).await {
                error!(task_id = %task_id, err = %e, "ytdlp job failed");
                if let Ok(mut t) = this.store.lock().unwrap().get_task(&task_id) {
                    t.status = TaskStatus::Failed;
                    t.error = Some(e.to_string());
                    t.updated_at = Utc::now();
                    let _ = this.save_and_emit(&t);
                }
            }
        });
    }

    async fn start_aria2_task(
        &self,
        task: &mut Task,
        cookies: Option<String>,
    ) -> Result<()> {
        let cfg = self.cfg.read().await;
        let dir = cfg.category_dir(&task.category).to_string_lossy().into_owned();
        let bt_settings = cfg.bt_settings();
        let referer = infer_referer(&task.url, task.referer.clone());
        drop(cfg);

        let lower = task.url.trim().to_ascii_lowercase();
        let filename = if router::is_direct_file_url(&lower) {
            task.filename.clone()
        } else {
            None
        };

        let opts = |referer: Option<String>, bt: bool| AddOptions {
            dir: dir.clone(),
            referer,
            filename: filename.clone(),
            cookies: cookies.clone(),
            bt,
            bt_settings: if bt { Some(bt_settings) } else { None },
        };

        let gid = if lower.starts_with("magnet:") {
            if let Some(hash) = magnet_infohash(&task.url) {
                if let Some(existing) = self.aria2.find_gid_by_infohash(&hash).await? {
                    existing
                } else {
                    match self.aria2.add_uri(vec![task.url.clone()], opts(None, true)).await {
                        Ok(gid) => gid,
                        Err(e) if e.to_string().contains("already registered") => self
                            .aria2
                            .find_gid_by_infohash(&hash)
                            .await?
                            .unwrap_or_default(),
                        Err(e) => return Err(e),
                    }
                }
            } else {
                self.aria2
                    .add_uri(vec![task.url.clone()], opts(None, true))
                    .await?
            }
        } else if is_torrent_url(&lower) {
            let b64 = fetch_torrent_b64(&task.url).await?;
            self.aria2.add_torrent(&b64, opts(referer, true)).await?
        } else {
            self.aria2
                .add_uri(vec![task.url.clone()], opts(referer, false))
                .await?
        };

        if !gid.is_empty() {
            task.backend_gid = Some(gid.clone());
        }
        task.status = TaskStatus::Downloading;
        task.updated_at = Utc::now();
        self.save_and_emit(task)?;
        info!(
            task_id = %task.id,
            gid = %gid,
            category = %task.category,
            url = %log_url(&task.url),
            "aria2 download started"
        );
        Ok(())
    }

    async fn run_ytdlp(self: &Arc<Self>, task_id: &str) -> Result<()> {
        let cookie_header = self.task_cookies.lock().unwrap().remove(task_id);

        let (url, referer, output_dir, quality, runner) = {
            let task = self.store.lock().unwrap().get_task(task_id)?;
            let cfg = self.cfg.read().await;
            let runner = YtdlpRunner {
                binary: cfg.ytdlp_path.clone(),
                ffmpeg_path: cfg.ffmpeg_path.clone(),
                aria2_secret: cfg.aria2_rpc_secret.clone(),
                use_aria2: true,
                quality: cfg.ytdlp_quality.clone(),
                cookies_file: cfg.ytdlp_cookies_file.clone(),
            };
            let dir = cfg.category_dir(&task.category);
            (
                task.url.clone(),
                task.referer.clone(),
                dir,
                task.quality.clone(),
                runner,
            )
        };

        {
            let mut task = self.store.lock().unwrap().get_task(task_id)?;
            task.status = TaskStatus::Downloading;
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;
            info!(task_id = %task_id, url = %log_url(&task.url), "yt-dlp download started");
        }

        let this = Arc::clone(self);
        let task_id_owned = task_id.to_string();
        let task_id_for_cb = task_id_owned.clone();
        let registry = self.ytdlp_jobs.clone();

        runner
            .download(
                DownloadOpts {
                    url: &url,
                    output_dir: &output_dir,
                    referer: referer.as_deref(),
                    quality: quality.as_deref(),
                    cookie_header: cookie_header.as_deref(),
                },
                Some(&registry),
                Some(task_id),
                move |p| {
                    if let Ok(mut task) = this.store.lock().unwrap().get_task(&task_id_for_cb) {
                        task.progress = p.percent;
                        task.done_bytes = p.done_bytes;
                        task.total_bytes = p.total_bytes;
                        task.speed = p.speed;
                        if let Some(name) = p.filename {
                            task.filename = Some(name);
                        }
        task.updated_at = Utc::now();
        let _ = this.save_and_emit(&task);
    }
},
            )
            .await?;

        let mut task = self.store.lock().unwrap().get_task(&task_id_owned)?;
        task.status = TaskStatus::Completed;
        task.progress = 100.0;
        task.completed_at = Some(Utc::now());
        task.updated_at = Utc::now();
        self.save_and_emit(&task)?;
        info!(
            task_id = %task_id_owned,
            filename = ?task.filename,
            bytes = task.done_bytes,
            "yt-dlp download completed"
        );

        let cfg = self.cfg.read().await;
        hooks::on_task_completed(&cfg, &task).await;
        Ok(())
    }

    async fn resolve_magnet_content_gid(
        &self,
        gid: &str,
        st: &aria2::Aria2Status,
        url: &str,
    ) -> Result<Option<String>> {
        if !st.followed_by.is_empty() {
            return Ok(Some(st.followed_by[0].clone()));
        }
        if let Some(next) = self.aria2.find_gid_following(gid).await? {
            return Ok(Some(next));
        }
        if aria2::is_metadata_status(st) {
            if let Some(hash) = magnet_infohash(url) {
                if let Some(next) = self.aria2.find_best_gid_for_magnet(&hash).await? {
                    if next != gid {
                        return Ok(Some(next));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn poll_aria2(self: &Arc<Self>) -> Result<()> {
        let active = self.store.lock().unwrap().active_tasks()?;
        for mut task in active {
            if task.task_type != TaskType::Aria2 {
                continue;
            }
            let gid = match task.backend_gid.clone() {
                Some(g) => g,
                None if task.url.starts_with("magnet:") => {
                    if let Some(hash) = magnet_infohash(&task.url) {
                        if let Some(g) = self.aria2.find_gid_by_infohash(&hash).await? {
                            task.backend_gid = Some(g.clone());
                            self.save_and_emit(&task)?;
                            g
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                None => continue,
            };
            let prev_status = task.status;
            let st = match self.aria2.tell_status(&gid).await {
                Ok(st) => st,
                Err(e) => {
                    if aria2_gid_not_found(&e) {
                        task.backend_gid = None;
                        if task.status != TaskStatus::Completed {
                            task.status = TaskStatus::Failed;
                            task.error = Some(
                                "aria2 lost track of this download (often after a container restart)"
                                    .into(),
                            );
                            task.updated_at = Utc::now();
                        }
                        self.save_and_emit(&task)?;
                        continue;
                    }
                    warn!(task_id = %task.id, gid = %gid, err = %e, "aria2 tell_status failed");
                    continue;
                }
            };
            let is_metadata = aria2::is_metadata_status(&st);
            let is_seeding = aria2::is_seeder(&st);

            // Magnet phase 1: metadata download → hand off to the real torrent gid.
            match self.resolve_magnet_content_gid(&gid, &st, &task.url).await {
                Ok(Some(next)) if next != gid => {
                    info!(
                        task_id = %task.id,
                        metadata_gid = %gid,
                        content_gid = %next,
                        "magnet metadata fetched, starting content download"
                    );
                    task.backend_gid = Some(next);
                    task.status = TaskStatus::Downloading;
                    task.progress = 0.0;
                    task.done_bytes = 0;
                    task.total_bytes = 0;
                    task.speed = 0;
                    task.upload_speed = 0;
                    task.connections = 0;
                    task.num_seeders = 0;
                    task.uploaded_bytes = 0;
                    task.seeding = false;
                    task.error = None;
                    task.completed_at = None;
                    task.filename = None;
                    task.updated_at = Utc::now();
                    self.save_and_emit(&task)?;
                    continue;
                }
                Err(e) => {
                    warn!(task_id = %task.id, err = %e, "magnet gid handoff failed");
                }
                _ => {}
            }

            let total = aria2::parse_i64(&st.total_length);
            let done = aria2::parse_i64(&st.completed_length);
            let speed = aria2::parse_i64(&st.download_speed);
            task.total_bytes = total;
            task.done_bytes = done;
            task.speed = speed;
            if is_bt_task(&task.url) {
                task.upload_speed = aria2::parse_i64(&st.upload_speed);
                task.connections = aria2::parse_i64(&st.connections) as i32;
                task.num_seeders = aria2::parse_i64(&st.num_seeders) as i32;
                task.uploaded_bytes = aria2::parse_i64(&st.upload_length);
            } else {
                task.upload_speed = 0;
                task.connections = 0;
                task.num_seeders = 0;
                task.uploaded_bytes = 0;
            }
            task.progress = if total > 0 {
                (done as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            if let Some(f) = st.files.first() {
                let path = std::path::Path::new(&f.path);
                task.filename = path.file_name().map(|s| s.to_string_lossy().into_owned());
                task.save_path = path.parent().map(|p| p.to_string_lossy().into_owned());
            }
            task.status = aria2::map_status(&st.status);
            let download_done = total > 0 && done >= total;
            if is_metadata {
                task.status = TaskStatus::Downloading;
                task.completed_at = None;
            } else if download_done {
                // aria2 stays "active" while seeding — treat a full download as complete.
                task.status = TaskStatus::Completed;
                task.completed_at = Some(task.completed_at.unwrap_or_else(Utc::now));
                task.progress = 100.0;
                task.speed = 0;
            }
            task.seeding =
                is_bt_task(&task.url) && download_done && !is_metadata && is_seeding;
            if !is_bt_task(&task.url) {
                task.seeding = false;
            }
            if task.status == TaskStatus::Failed && !st.error_message.is_empty() {
                task.error = Some(st.error_message);
            }
            if task.status == TaskStatus::Completed && !is_metadata && task.completed_at.is_none() {
                task.completed_at = Some(Utc::now());
                task.progress = 100.0;
            }
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;

            if task.status == TaskStatus::Completed && prev_status != TaskStatus::Completed {
                if is_metadata {
                    continue;
                }
                info!(
                    task_id = %task.id,
                    filename = ?task.filename,
                    bytes = task.done_bytes,
                    seeding = is_seeding,
                    upload_speed = task.upload_speed,
                    peers = task.connections,
                    "download completed"
                );
                let cfg = self.cfg.read().await;
                hooks::on_task_completed(&cfg, &task).await;
            }

            if task.status == TaskStatus::Failed && prev_status != TaskStatus::Failed {
                info!(
                    task_id = %task.id,
                    error = ?task.error,
                    url = %log_url(&task.url),
                    "download failed"
                );
                let lower = task.url.trim().to_ascii_lowercase();
                if lower.starts_with("magnet:") || is_torrent_url(&lower) {
                    continue;
                }
                if router::skip_ytdlp_simulate(&lower) {
                    continue;
                }
                let runner = self.ytdlp_runner().await;
                if runner.simulate(&task.url).await.unwrap_or(false) {
                    task.task_type = TaskType::Ytdlp;
                    task.status = TaskStatus::Pending;
                    task.error = None;
                    task.backend_gid = None;
                    task.updated_at = Utc::now();
                    self.save_and_emit(&task)?;
                    self.spawn_ytdlp(task.id.clone());
                }
            }
        }

        // Keep seeding stats fresh for completed torrents still in aria2.
        let completed = self.store.lock().unwrap().list_tasks(50, Some("completed"))?;
        for mut task in completed {
            if task.task_type != TaskType::Aria2 || !is_bt_task(&task.url) {
                continue;
            }
            let Some(gid) = task.backend_gid.clone() else {
                continue;
            };
            let st = match self.aria2.tell_status(&gid).await {
                Ok(st) => st,
                Err(e) => {
                    if aria2_gid_not_found(&e) {
                        task.backend_gid = None;
                        task.seeding = false;
                        task.upload_speed = 0;
                        task.updated_at = Utc::now();
                        self.save_and_emit(&task)?;
                    } else if task.seeding || task.upload_speed > 0 {
                        task.seeding = false;
                        task.upload_speed = 0;
                        task.updated_at = Utc::now();
                        self.save_and_emit(&task)?;
                    }
                    continue;
                }
            };
            let total = aria2::parse_i64(&st.total_length);
            let done = aria2::parse_i64(&st.completed_length);
            let download_done = total > 0 && done >= total;
            let is_metadata = aria2::is_metadata_status(&st);
            let is_seeding = aria2::is_seeder(&st);
            task.seeding = download_done && !is_metadata && is_seeding;
            task.upload_speed = aria2::parse_i64(&st.upload_speed);
            task.connections = aria2::parse_i64(&st.connections) as i32;
            task.num_seeders = aria2::parse_i64(&st.num_seeders) as i32;
            task.uploaded_bytes = aria2::parse_i64(&st.upload_length);
            if !task.seeding {
                task.upload_speed = 0;
            }
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;
        }

        Ok(())
    }

    pub async fn pause_task(&self, id: &str) -> Result<Task> {
        let mut task = self.store.lock().unwrap().get_task(id)?;
        if task.task_type == TaskType::Aria2 {
            if let Some(gid) = &task.backend_gid {
                self.aria2.pause(gid).await?;
            }
        } else if task.task_type == TaskType::Ytdlp {
            self.ytdlp_jobs.cancel(id).await;
        }
        task.status = TaskStatus::Paused;
        task.updated_at = Utc::now();
        self.save_and_emit(&task)?;
        Ok(task)
    }

    pub async fn resume_task(self: &Arc<Self>, id: &str) -> Result<Task> {
        let mut task = self.store.lock().unwrap().get_task(id)?;
        if task.task_type == TaskType::Aria2 {
            if task.url.starts_with("magnet:") {
                if let Some(hash) = magnet_infohash(&task.url) {
                    if let Some(gid) = self.aria2.find_best_gid_for_magnet(&hash).await? {
                        task.backend_gid = Some(gid);
                    }
                }
            }
            if let Some(gid) = &task.backend_gid {
                self.aria2.unpause(gid).await?;
            }
            task.status = TaskStatus::Downloading;
            task.completed_at = None;
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;
            Ok(task)
        } else {
            task.status = TaskStatus::Downloading;
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;
            self.spawn_ytdlp(task.id.clone());
            Ok(task)
        }
    }

    pub async fn remove_task(&self, id: &str) -> Result<()> {
        let task = self.store.lock().unwrap().get_task(id)?;
        if task.task_type == TaskType::Aria2 {
            if let Some(gid) = task.backend_gid {
                let _ = self.aria2.remove(&gid).await;
            }
        } else if task.task_type == TaskType::Ytdlp {
            self.ytdlp_jobs.cancel(id).await;
        }
        self.store.lock().unwrap().delete_task(id)?;
        Ok(())
    }

    pub async fn update_ytdlp(&self) -> Result<String> {
        let cfg = self.cfg.read().await;
        let runner = YtdlpRunner {
            binary: cfg.ytdlp_path.clone(),
            ffmpeg_path: cfg.ffmpeg_path.clone(),
            aria2_secret: cfg.aria2_rpc_secret.clone(),
            use_aria2: false,
            quality: cfg.ytdlp_quality.clone(),
            cookies_file: cfg.ytdlp_cookies_file.clone(),
        };
        drop(cfg);
        let msg = runner.update().await?;
        self.version_checker.invalidate().await;
        info!(message = %msg, "yt-dlp updated");
        Ok(msg)
    }
}

fn infer_referer(url: &str, referer: Option<String>) -> Option<String> {
    if referer.is_some() {
        return referer;
    }
    let lower = url.to_ascii_lowercase();
    if lower.contains("akirabox.") {
        return Some("https://akirabox.to/".into());
    }
    url::Url::parse(url.trim())
        .ok()
        .map(|u| format!("{}/", u.origin().ascii_serialization()))
}

fn log_url(url: &str) -> String {
    let u = url.trim();
    if u.len() <= 100 {
        u.to_string()
    } else {
        format!("{}…", &u[..100])
    }
}

fn magnet_infohash(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let pos = lower.find("xt=urn:btih:")?;
    let hash = lower[pos + 12..].split('&').next()?.trim();
    if hash.len() >= 32 {
        Some(hash.to_string())
    } else {
        None
    }
}

fn is_torrent_url(lower: &str) -> bool {
    lower.ends_with(".torrent")
        || lower.contains(".torrent?")
        || lower.contains(".torrent&")
}

fn is_bt_task(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("magnet:") || is_torrent_url(&lower)
}

fn aria2_gid_not_found(err: &impl std::fmt::Display) -> bool {
    err.to_string().to_ascii_lowercase().contains("not found")
}

async fn fetch_torrent_b64(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build torrent client")?;
    let bytes = client
        .get(url)
        .send()
        .await
        .context("fetch torrent")?
        .error_for_status()
        .context("torrent HTTP error")?
        .bytes()
        .await
        .context("read torrent body")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}
