use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Aria2Client {
    url: String,
    secret: String,
    client: Client,
    id: Arc<AtomicU64>,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct VersionResult {
    version: String,
}

#[derive(Debug, Deserialize)]
pub struct Aria2Status {
    #[serde(rename = "gid")]
    pub _gid: String,
    pub status: String,
    #[serde(rename = "totalLength")]
    pub total_length: String,
    #[serde(rename = "completedLength")]
    pub completed_length: String,
    #[serde(rename = "downloadSpeed")]
    pub download_speed: String,
    pub files: Vec<Aria2File>,
    #[serde(rename = "errorMessage", default)]
    pub error_message: String,
}

#[derive(Debug, Deserialize)]
pub struct Aria2File {
    pub path: String,
}

pub struct AddOptions {
    pub dir: String,
    pub referer: Option<String>,
    pub filename: Option<String>,
}

impl Aria2Client {
    pub fn new(url: String, secret: String) -> Self {
        Self {
            url,
            secret,
            client: Client::new(),
            id: Arc::new(AtomicU64::new(0)),
        }
    }

    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        let mut all_params = Vec::new();
        if !self.secret.is_empty() {
            all_params.push(json!(format!("token:{}", self.secret)));
        }
        all_params.extend(params);

        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id.to_string(),
            "method": method,
            "params": all_params,
        });

        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .context("aria2 rpc request")?;

        let parsed: RpcResponse = resp.json().await.context("aria2 rpc parse")?;
        if let Some(err) = parsed.error {
            anyhow::bail!("aria2 {}: {}", method, err.message);
        }
        parsed.result.context("aria2 empty result")
    }

    pub async fn ping(&self) -> Result<()> {
        self.call("aria2.getVersion", vec![]).await?;
        Ok(())
    }

    pub async fn version(&self) -> Result<String> {
        let result = self.call("aria2.getVersion", vec![]).await?;
        let v: VersionResult = serde_json::from_value(result)?;
        Ok(v.version)
    }

    pub async fn add_uri(&self, uris: Vec<String>, opts: AddOptions) -> Result<String> {
        let mut options = json!({ "dir": opts.dir });
        if let Some(r) = opts.referer {
            options["referer"] = json!(r);
        }
        if let Some(f) = opts.filename {
            options["out"] = json!(f);
        }
        let result = self
            .call("aria2.addUri", vec![json!(uris), options])
            .await?;
        result
            .as_str()
            .map(String::from)
            .context("aria2 gid not string")
    }

    pub async fn add_torrent(&self, torrent_b64: &str, opts: AddOptions) -> Result<String> {
        let options = json!({ "dir": opts.dir });
        let result = self
            .call(
                "aria2.addTorrent",
                vec![json!(torrent_b64), json!([]), options],
            )
            .await?;
        result
            .as_str()
            .map(String::from)
            .context("aria2 gid not string")
    }

    pub async fn tell_status(&self, gid: &str) -> Result<Aria2Status> {
        let result = self.call("aria2.tellStatus", vec![json!(gid)]).await?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn pause(&self, gid: &str) -> Result<()> {
        self.call("aria2.pause", vec![json!(gid)]).await?;
        Ok(())
    }

    pub async fn unpause(&self, gid: &str) -> Result<()> {
        self.call("aria2.unpause", vec![json!(gid)]).await?;
        Ok(())
    }

    pub async fn remove(&self, gid: &str) -> Result<()> {
        self.call("aria2.remove", vec![json!(gid)]).await?;
        Ok(())
    }
}

pub fn map_status(s: &str) -> crate::store::TaskStatus {
    match s.to_ascii_lowercase().as_str() {
        "complete" => crate::store::TaskStatus::Completed,
        "error" => crate::store::TaskStatus::Failed,
        "paused" => crate::store::TaskStatus::Paused,
        _ => crate::store::TaskStatus::Downloading,
    }
}

pub fn parse_i64(s: &str) -> i64 {
    s.parse().unwrap_or(0)
}
