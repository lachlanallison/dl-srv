use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Aria2,
    Ytdlp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Downloading,
    Paused,
    Completed,
    Failed,
    Removed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Removed => "removed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "removed" => Self::Removed,
            _ => Self::Pending,
        }
    }
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aria2 => "aria2",
            Self::Ytdlp => "ytdlp",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ytdlp" => Self::Ytdlp,
            _ => Self::Aria2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub url: String,
    #[serde(rename = "type")]
    pub task_type: TaskType,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_gid: Option<String>,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    pub progress: f64,
    pub done_bytes: i64,
    pub total_bytes: i64,
    pub speed: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssFeed {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub filter_regex: Option<String>,
    pub category: String,
    pub enabled: bool,
    pub poll_interval_secs: i64,
    pub created_at: DateTime<Utc>,
}

pub struct AddTaskInput {
    pub url: String,
    pub category: String,
    pub referer: Option<String>,
    pub force_ytdlp: bool,
    pub quality: Option<String>,
    pub source: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                type TEXT NOT NULL,
                status TEXT NOT NULL,
                backend_gid TEXT,
                category TEXT NOT NULL DEFAULT 'inbox',
                filename TEXT,
                save_path TEXT,
                progress REAL NOT NULL DEFAULT 0,
                done_bytes INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER NOT NULL DEFAULT 0,
                speed INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                referer TEXT,
                quality TEXT,
                source TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at DESC);
            CREATE TABLE IF NOT EXISTS rss_feeds (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                filter_regex TEXT,
                category TEXT NOT NULL DEFAULT 'inbox',
                enabled INTEGER NOT NULL DEFAULT 1,
                poll_interval_secs INTEGER NOT NULL DEFAULT 300,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rss_seen (
                feed_id TEXT NOT NULL,
                guid TEXT NOT NULL,
                seen_at TEXT NOT NULL,
                PRIMARY KEY (feed_id, guid)
            );
            ",
        )?;
        migrate_tasks(&conn)?;
        Ok(Self { conn })
    }

    pub fn create_task(&self, input: &AddTaskInput, task_type: TaskType, save_path: &str) -> Result<Task> {
        let now = Utc::now();
        let category = if input.category.is_empty() {
            "inbox".to_string()
        } else {
            input.category.clone()
        };
        let task = Task {
            id: Uuid::new_v4().to_string(),
            url: input.url.clone(),
            task_type,
            status: TaskStatus::Pending,
            backend_gid: None,
            category,
            filename: None,
            save_path: Some(save_path.to_string()),
            progress: 0.0,
            done_bytes: 0,
            total_bytes: 0,
            speed: 0,
            error: None,
            referer: input.referer.clone(),
            quality: input.quality.clone(),
            source: input.source.clone(),
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        self.conn.execute(
            "INSERT INTO tasks (id, url, type, status, category, save_path, referer, quality, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                task.id,
                task.url,
                task.task_type.as_str(),
                task.status.as_str(),
                task.category,
                task.save_path,
                task.referer,
                task.quality,
                task.source,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(task)
    }

    pub fn list_tasks(&self, limit: i64, status: Option<&str>) -> Result<Vec<Task>> {
        const COLS: &str = "id, url, type, status, backend_gid, category, filename, save_path,
                    progress, done_bytes, total_bytes, speed, error, referer, quality, source,
                    created_at, updated_at, completed_at";
        let mut stmt = match status {
            Some("active") => self.conn.prepare(&format!(
                "SELECT {COLS} FROM tasks WHERE status IN ('pending', 'downloading', 'paused')
                 ORDER BY created_at DESC LIMIT ?1"
            ))?,
            Some(_) => self.conn.prepare(&format!(
                "SELECT {COLS} FROM tasks WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2"
            ))?,
            None => self.conn.prepare(&format!(
                "SELECT {COLS} FROM tasks ORDER BY created_at DESC LIMIT ?1"
            ))?,
        };
        let rows = match status {
            Some("active") => stmt.query_map([limit], row_to_task)?,
            Some(st) => stmt.query_map(params![st, limit], row_to_task)?,
            None => stmt.query_map([limit], row_to_task)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_task(&self, id: &str) -> Result<Task> {
        self.conn
            .query_row(
                "SELECT id, url, type, status, backend_gid, category, filename, save_path,
                        progress, done_bytes, total_bytes, speed, error, referer, quality, source,
                        created_at, updated_at, completed_at
                 FROM tasks WHERE id = ?1",
                [id],
                row_to_task,
            )
            .with_context(|| format!("task {id} not found"))
    }

    pub fn active_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, type, status, backend_gid, category, filename, save_path,
                    progress, done_bytes, total_bytes, speed, error, referer, quality, source,
                    created_at, updated_at, completed_at
             FROM tasks WHERE status IN ('pending', 'downloading', 'paused')",
        )?;
        let rows = stmt.query_map([], row_to_task)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET status=?1, backend_gid=?2, filename=?3, save_path=?4, progress=?5,
             done_bytes=?6, total_bytes=?7, speed=?8, error=?9, quality=?10, source=?11,
             updated_at=?12, completed_at=?13
             WHERE id=?14",
            params![
                task.status.as_str(),
                task.backend_gid,
                task.filename,
                task.save_path,
                task.progress,
                task.done_bytes,
                task.total_bytes,
                task.speed,
                task.error,
                task.quality,
                task.source,
                task.updated_at.to_rfc3339(),
                task.completed_at.map(|t| t.to_rfc3339()),
                task.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let n = self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        if n == 0 {
            anyhow::bail!("task not found");
        }
        Ok(())
    }

    pub fn list_rss_feeds(&self) -> Result<Vec<RssFeed>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, filter_regex, category, enabled, poll_interval_secs, created_at
             FROM rss_feeds ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RssFeed {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                filter_regex: row.get(3)?,
                category: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                poll_interval_secs: row.get(6)?,
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_rss_feed(&self, id: &str) -> Result<RssFeed> {
        self.conn
            .query_row(
                "SELECT id, url, title, filter_regex, category, enabled, poll_interval_secs, created_at
                 FROM rss_feeds WHERE id = ?1",
                [id],
                |row| {
                    Ok(RssFeed {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        title: row.get(2)?,
                        filter_regex: row.get(3)?,
                        category: row.get(4)?,
                        enabled: row.get::<_, i64>(5)? != 0,
                        poll_interval_secs: row.get(6)?,
                        created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    })
                },
            )
            .with_context(|| format!("rss feed {id} not found"))
    }

    pub fn create_rss_feed(
        &self,
        url: &str,
        title: Option<&str>,
        filter_regex: Option<&str>,
        category: &str,
        enabled: bool,
        poll_interval_secs: i64,
    ) -> Result<RssFeed> {
        let now = Utc::now();
        let feed = RssFeed {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            title: title.map(String::from),
            filter_regex: filter_regex.map(String::from),
            category: if category.is_empty() {
                "inbox".into()
            } else {
                category.to_string()
            },
            enabled,
            poll_interval_secs,
            created_at: now,
        };
        self.conn.execute(
            "INSERT INTO rss_feeds (id, url, title, filter_regex, category, enabled, poll_interval_secs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                feed.id,
                feed.url,
                feed.title,
                feed.filter_regex,
                feed.category,
                feed.enabled as i64,
                feed.poll_interval_secs,
                feed.created_at.to_rfc3339(),
            ],
        )?;
        Ok(feed)
    }

    pub fn update_rss_feed(&self, feed: &RssFeed) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE rss_feeds SET url=?1, title=?2, filter_regex=?3, category=?4,
             enabled=?5, poll_interval_secs=?6 WHERE id=?7",
            params![
                feed.url,
                feed.title,
                feed.filter_regex,
                feed.category,
                feed.enabled as i64,
                feed.poll_interval_secs,
                feed.id,
            ],
        )?;
        if n == 0 {
            anyhow::bail!("rss feed not found");
        }
        Ok(())
    }

    pub fn delete_rss_feed(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM rss_seen WHERE feed_id = ?1", [id])?;
        let n = self.conn.execute("DELETE FROM rss_feeds WHERE id = ?1", [id])?;
        if n == 0 {
            anyhow::bail!("rss feed not found");
        }
        Ok(())
    }

    pub fn rss_seen(&self, feed_id: &str, guid: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM rss_seen WHERE feed_id = ?1 AND guid = ?2",
            params![feed_id, guid],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn mark_rss_seen(&self, feed_id: &str, guid: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO rss_seen (feed_id, guid, seen_at) VALUES (?1, ?2, ?3)",
            params![feed_id, guid, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn enabled_rss_feeds(&self) -> Result<Vec<RssFeed>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, filter_regex, category, enabled, poll_interval_secs, created_at
             FROM rss_feeds WHERE enabled = 1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RssFeed {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                filter_regex: row.get(3)?,
                category: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                poll_interval_secs: row.get(6)?,
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn migrate_tasks(conn: &Connection) -> Result<()> {
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(tasks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .collect();
    if !cols.iter().any(|c| c == "quality") {
        conn.execute("ALTER TABLE tasks ADD COLUMN quality TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "source") {
        conn.execute("ALTER TABLE tasks ADD COLUMN source TEXT", [])?;
    }
    Ok(())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        url: row.get(1)?,
        task_type: TaskType::parse(row.get::<_, String>(2)?.as_str()),
        status: TaskStatus::parse(row.get::<_, String>(3)?.as_str()),
        backend_gid: row.get(4)?,
        category: row.get(5)?,
        filename: row.get(6)?,
        save_path: row.get(7)?,
        progress: row.get(8)?,
        done_bytes: row.get(9)?,
        total_bytes: row.get(10)?,
        speed: row.get(11)?,
        error: row.get(12)?,
        referer: row.get(13)?,
        quality: row.get(14)?,
        source: row.get(15)?,
        created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(16)?)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(17)?)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        completed_at: row
            .get::<_, Option<String>>(18)?
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
    })
}
