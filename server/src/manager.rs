use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, warn};

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

    pub async fn add_task(self: &Arc<Self>, input: AddTaskInput) -> Result<Task> {
        let runner = self.ytdlp_runner().await;
        let cfg = self.cfg.read().await;
        let category = if input.category.is_empty() {
            cfg.default_category.clone()
        } else {
            input.category.clone()
        };
        let save_dir = config::ensure_category_dir(&cfg, &category)?;
        let save_path = save_dir.to_string_lossy().into_owned();
        drop(cfg);

        let task_type = router::classify(&input.url, input.force_ytdlp, Some(&runner)).await;

        let mut task = self
            .store
            .lock()
            .unwrap()
            .create_task(&input, task_type, &save_path)?;

        match task_type {
            TaskType::Aria2 => {
                if let Err(e) = self.start_aria2_task(&mut task).await {
                    if runner.simulate(&task.url).await.unwrap_or(false) {
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

        Ok(task)
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

    async fn start_aria2_task(&self, task: &mut Task) -> Result<()> {
        let cfg = self.cfg.read().await;
        let dir = cfg.category_dir(&task.category).to_string_lossy().into_owned();
        let referer = infer_referer(&task.url, task.referer.clone());
        drop(cfg);

        let lower = task.url.trim().to_ascii_lowercase();
        let gid = if lower.starts_with("magnet:") {
            self.aria2
                .add_uri(
                    vec![task.url.clone()],
                    AddOptions {
                        dir,
                        referer,
                        filename: None,
                    },
                )
                .await?
        } else if is_torrent_url(&lower) {
            let b64 = fetch_torrent_b64(&task.url).await?;
            self.aria2
                .add_torrent(
                    &b64,
                    AddOptions {
                        dir,
                        referer,
                        filename: None,
                    },
                )
                .await?
        } else {
            self.aria2
                .add_uri(
                    vec![task.url.clone()],
                    AddOptions {
                        dir,
                        referer,
                        filename: None,
                    },
                )
                .await?
        };

        task.backend_gid = Some(gid);
        task.status = TaskStatus::Downloading;
        task.updated_at = Utc::now();
        self.save_and_emit(task)?;
        Ok(())
    }

    async fn run_ytdlp(self: &Arc<Self>, task_id: &str) -> Result<()> {
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
                        let _ = this.store.lock().unwrap().update_task(&task);
                        this.emit(&task);
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

        let cfg = self.cfg.read().await;
        hooks::on_task_completed(&cfg, &task).await;
        Ok(())
    }

    async fn poll_aria2(self: &Arc<Self>) -> Result<()> {
        let active = self.store.lock().unwrap().active_tasks()?;
        for mut task in active {
            if task.task_type != TaskType::Aria2 {
                continue;
            }
            let Some(gid) = task.backend_gid.clone() else {
                continue;
            };
            let prev_status = task.status;
            let st = self.aria2.tell_status(&gid).await?;
            let total = aria2::parse_i64(&st.total_length);
            let done = aria2::parse_i64(&st.completed_length);
            let speed = aria2::parse_i64(&st.download_speed);
            task.total_bytes = total;
            task.done_bytes = done;
            task.speed = speed;
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
            if task.status == TaskStatus::Failed && !st.error_message.is_empty() {
                task.error = Some(st.error_message);
            }
            if task.status == TaskStatus::Completed {
                task.completed_at = Some(Utc::now());
                task.progress = 100.0;
            }
            task.updated_at = Utc::now();
            self.save_and_emit(&task)?;

            if task.status == TaskStatus::Completed && prev_status != TaskStatus::Completed {
                let cfg = self.cfg.read().await;
                hooks::on_task_completed(&cfg, &task).await;
            }

            if task.status == TaskStatus::Failed && prev_status != TaskStatus::Failed {
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
            if let Some(gid) = &task.backend_gid {
                self.aria2.unpause(gid).await?;
            }
            task.status = TaskStatus::Downloading;
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
        Ok(msg)
    }
}

fn infer_referer(url: &str, referer: Option<String>) -> Option<String> {
    if referer.is_some() {
        return referer;
    }
    url::Url::parse(url.trim())
        .ok()
        .map(|u| format!("{}/", u.origin().ascii_serialization()))
}

fn is_torrent_url(lower: &str) -> bool {
    lower.ends_with(".torrent")
        || lower.contains(".torrent?")
        || lower.contains(".torrent&")
}

async fn fetch_torrent_b64(url: &str) -> Result<String> {
    let bytes = reqwest::get(url)
        .await
        .context("fetch torrent")?
        .error_for_status()
        .context("torrent HTTP error")?
        .bytes()
        .await
        .context("read torrent body")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}
