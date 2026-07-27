use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

pub struct YtdlpRunner {
    pub binary: String,
    pub ffmpeg_path: String,
    pub aria2_secret: String,
    pub use_aria2: bool,
    pub quality: String,
    pub cookies_file: Option<String>,
}

pub struct DownloadOpts<'a> {
    pub url: &'a str,
    pub output_dir: &'a Path,
    pub referer: Option<&'a str>,
    pub quality: Option<&'a str>,
}

pub struct Progress {
    pub percent: f64,
    pub done_bytes: i64,
    pub total_bytes: i64,
    pub speed: i64,
    pub filename: Option<String>,
}

pub struct YtdlpJobRegistry {
    jobs: Mutex<HashMap<String, Child>>,
}

impl Default for YtdlpJobRegistry {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

impl YtdlpJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, task_id: &str, child: Child) {
        self.jobs
            .lock()
            .unwrap()
            .insert(task_id.to_string(), child);
    }

    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut child = self.jobs.lock().unwrap().remove(task_id);
        if let Some(ref mut c) = child {
            let _ = c.start_kill();
            let _ = c.wait().await;
            true
        } else {
            false
        }
    }

    pub fn remove(&self, task_id: &str) {
        self.jobs.lock().unwrap().remove(task_id);
    }

    pub fn take(&self, task_id: &str) -> Option<Child> {
        self.jobs.lock().unwrap().remove(task_id)
    }
}

pub fn quality_format(quality: &str) -> String {
    match quality {
        "best" => "bestvideo+bestaudio/best".into(),
        "1080" | "1080p" => "bestvideo[height<=1080]+bestaudio/best[height<=1080]".into(),
        "720" | "720p" => "bestvideo[height<=720]+bestaudio/best[height<=720]".into(),
        "480" | "480p" => "bestvideo[height<=480]+bestaudio/best[height<=480]".into(),
        "audio" => "bestaudio/best".into(),
        other => other.to_string(),
    }
}

impl YtdlpRunner {
    pub async fn version(&self) -> Result<String> {
        let out = Command::new(&self.binary)
            .arg("--version")
            .output()
            .await
            .context("run yt-dlp --version")?;
        let line = String::from_utf8_lossy(&out.stdout);
        Ok(line.lines().next().unwrap_or("").trim().to_string())
    }

    pub async fn update(&self) -> Result<String> {
        let out = Command::new(&self.binary)
            .arg("-U")
            .output()
            .await
            .context("run yt-dlp -U")?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub async fn simulate(&self, url: &str) -> Result<bool> {
        let out = Command::new(&self.binary)
            .args(["--simulate", "--no-playlist", url])
            .output()
            .await
            .context("run yt-dlp --simulate")?;
        Ok(out.status.success())
    }

    pub async fn download<F>(
        &self,
        opts: DownloadOpts<'_>,
        registry: Option<&YtdlpJobRegistry>,
        task_id: Option<&str>,
        mut on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(Progress),
    {
        tokio::fs::create_dir_all(opts.output_dir)
            .await
            .context("create output dir")?;

        let output_template = opts
            .output_dir
            .join("%(title)s.%(ext)s")
            .to_string_lossy()
            .into_owned();

        let quality = opts.quality.unwrap_or(&self.quality);
        let format = quality_format(quality);

        let mut cmd = Command::new(&self.binary);
        cmd.args([
            "--newline",
            "--no-playlist",
            "--continue",
            "-f",
            &format,
            "-o",
            &output_template,
            "--print",
            "after_move:filepath",
        ]);

        if self.use_aria2 {
            let mut aria2_args = "-x 16 -s 16 -k 1M".to_string();
            if !self.aria2_secret.is_empty() {
                aria2_args.push_str(&format!(" --rpc-secret={}", self.aria2_secret));
            }
            cmd.arg("--external-downloader")
                .arg("aria2c")
                .arg("--external-downloader-args")
                .arg(aria2_args);
        }

        if let Some(r) = opts.referer {
            cmd.arg("--referer").arg(r);
        }

        if let Some(ref cookies) = self.cookies_file {
            if !cookies.is_empty() {
                cmd.arg("--cookies").arg(cookies);
            }
        }

        if !self.ffmpeg_path.is_empty() {
            if let Some(dir) = Path::new(&self.ffmpeg_path).parent() {
                let sep = std::path::MAIN_SEPARATOR_STR;
                let path = std::env::var("PATH").unwrap_or_default();
                cmd.env("PATH", format!("{}{}{}", dir.display(), sep, path));
            }
        }

        cmd.arg(opts.url);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        let mut child = cmd.spawn().context("spawn yt-dlp")?;
        let stdout = child.stdout.take().context("yt-dlp stdout")?;
        let local_child = if let (Some(reg), Some(tid)) = (registry, task_id) {
            reg.register(tid, child);
            None
        } else {
            Some(child)
        };
        let progress_re = Regex::new(
            r"\[download\]\s+([\d.]+)%\s+of\s+([\d.]+)(KiB|MiB|GiB|TiB|B)\s+at\s+([\d.]+)(KiB|MiB|GiB|TiB|B)/s",
        )
        .unwrap();

        let mut reader = BufReader::new(stdout).lines();
        let mut final_file: Option<String> = None;

        while let Some(line) = reader.next_line().await? {
            if line.is_empty() {
                continue;
            }
            if !line.starts_with('[') && Path::new(&line).is_absolute() {
                final_file = Some(line.clone());
                on_progress(Progress {
                    percent: 100.0,
                    done_bytes: 0,
                    total_bytes: 0,
                    speed: 0,
                    filename: Some(
                        Path::new(&line)
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ),
                });
                continue;
            }
            if line.starts_with("[download]") {
                if let Some(caps) = progress_re.captures(&line) {
                    let pct: f64 = caps[1].parse().unwrap_or(0.0);
                    let total = size_to_bytes(&caps[2], &caps[3]);
                    let speed = size_to_bytes(&caps[4], &caps[5]);
                    let done = ((total as f64) * pct / 100.0) as i64;
                    on_progress(Progress {
                        percent: pct,
                        done_bytes: done,
                        total_bytes: total,
                        speed,
                        filename: final_file.as_ref().and_then(|p| {
                            Path::new(p)
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                        }),
                    });
                }
            }
        }

        let status = if let (Some(reg), Some(tid)) = (registry, task_id) {
            let mut child = reg
                .take(tid)
                .context("yt-dlp child missing from registry")?;
            child.wait().await?
        } else if let Some(mut child) = local_child {
            child.wait().await?
        } else {
            anyhow::bail!("yt-dlp child handle missing");
        };
        if !status.success() {
            anyhow::bail!("yt-dlp exited with {status}");
        }
        Ok(())
    }
}

fn size_to_bytes(value: &str, unit: &str) -> i64 {
    let v: f64 = value.parse().unwrap_or(0.0);
    let mult = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (v * mult) as i64
}

pub async fn ffmpeg_version(path: &str) -> Result<String> {
    let out = Command::new(path).arg("-version").output().await?;
    let line = String::from_utf8_lossy(&out.stdout);
    Ok(line.lines().next().unwrap_or("").trim().to_string())
}
