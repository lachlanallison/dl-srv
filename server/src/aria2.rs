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
    #[serde(rename = "uploadSpeed", default)]
    pub upload_speed: String,
    #[serde(rename = "connections", default)]
    pub connections: String,
    #[serde(rename = "numSeeders", default)]
    pub num_seeders: String,
    #[serde(rename = "uploadLength", default)]
    pub upload_length: String,
    pub files: Vec<Aria2File>,
    #[serde(rename = "errorMessage", default)]
    pub error_message: String,
    #[serde(rename = "followedBy", default)]
    pub followed_by: Vec<String>,
    #[serde(rename = "following", default)]
    pub following: String,
    #[serde(rename = "infoHash", default)]
    pub info_hash: String,
    #[serde(rename = "seeder", default)]
    pub seeder: String,
}

#[derive(Debug, Deserialize)]
pub struct Aria2File {
    pub path: String,
}

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub struct AddOptions {
    pub dir: String,
    pub referer: Option<String>,
    pub filename: Option<String>,
    pub cookies: Option<String>,
    /// Magnet/torrent: skip HTTP headers, enable bt-save-metadata.
    pub bt: bool,
    pub bt_settings: Option<BtSettings>,
}

#[derive(Debug, Clone, Copy)]
pub struct BtSettings {
    pub seed_ratio: f64,
    pub seed_time: u32,
    pub max_peers: u32,
    pub max_upload_limit: i64,
    pub max_download_limit: i64,
}

impl BtSettings {
    fn option_map(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("seed-ratio".into(), json!(format!("{:.2}", self.seed_ratio)));
        map.insert("seed-time".into(), json!(self.seed_time.to_string()));
        map.insert("bt-max-peers".into(), json!(self.max_peers.to_string()));
        map.insert(
            "max-upload-limit".into(),
            json!(self.max_upload_limit.to_string()),
        );
        map.insert(
            "max-download-limit".into(),
            json!(self.max_download_limit.to_string()),
        );
        map
    }
}

fn download_options(opts: AddOptions) -> Value {
    let mut options = json!({ "dir": opts.dir });
    if opts.bt {
        options["bt-save-metadata"] = json!("true");
        if let Some(bt) = opts.bt_settings {
            if let Value::Object(ref mut base) = options {
                base.extend(bt.option_map());
            }
        }
        return options;
    }
    let mut headers = vec![format!("User-Agent: {BROWSER_UA}")];
    if let Some(c) = opts.cookies.filter(|c| !c.is_empty()) {
        headers.push(format!("Cookie: {c}"));
    }
    options["header"] = json!(headers);
    if let Some(r) = opts.referer {
        options["referer"] = json!(r);
    }
    if let Some(f) = opts.filename {
        options["out"] = json!(f);
    }
    options
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
        let options = download_options(opts);
        let result = self
            .call("aria2.addUri", vec![json!(uris), options])
            .await?;
        result
            .as_str()
            .map(String::from)
            .context("aria2 gid not string")
    }

    pub async fn add_torrent(&self, torrent_b64: &str, opts: AddOptions) -> Result<String> {
        let options = download_options(opts);
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

    pub async fn change_global_bt_options(&self, bt: &BtSettings) -> Result<()> {
        self.call(
            "aria2.changeGlobalOption",
            vec![Value::Object(bt.option_map())],
        )
        .await?;
        Ok(())
    }

    pub async fn tell_status(&self, gid: &str) -> Result<Aria2Status> {
        let keys = json!([
            "gid",
            "status",
            "totalLength",
            "completedLength",
            "downloadSpeed",
            "uploadSpeed",
            "uploadLength",
            "connections",
            "numSeeders",
            "seeder",
            "files",
            "errorMessage",
            "followedBy",
            "following",
            "infoHash"
        ]);
        let result = self
            .call("aria2.tellStatus", vec![json!(gid), keys])
            .await?;
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

    async fn list_gids(&self) -> Result<Vec<String>> {
        let mut gids = Vec::new();
        // Paused downloads are included in tellWaiting — there is no tellPaused RPC.
        let calls: [(&str, Vec<Value>); 2] = [
            ("aria2.tellActive", vec![]),
            ("aria2.tellWaiting", vec![json!(0), json!(1000)]),
        ];
        for (method, params) in calls {
            let result = self.call(method, params).await?;
            if let Some(arr) = result.as_array() {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        gids.push(s.to_string());
                    } else if let Some(gid) = v.get("gid").and_then(|g| g.as_str()) {
                        gids.push(gid.to_string());
                    }
                }
            }
        }
        Ok(gids)
    }

    pub async fn find_gid_following(&self, parent_gid: &str) -> Result<Option<String>> {
        for gid in self.list_gids().await? {
            let st = self.tell_status(&gid).await?;
            if st.following == parent_gid {
                return Ok(Some(gid));
            }
        }
        Ok(None)
    }

    pub async fn find_best_gid_for_magnet(&self, infohash: &str) -> Result<Option<String>> {
        let want = infohash.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Ok(None);
        }

        let mut content: Vec<(String, i64)> = Vec::new();
        let mut metadata_gid: Option<String> = None;
        for gid in self.list_gids().await? {
            let st = self.tell_status(&gid).await?;
            if st.info_hash.to_ascii_lowercase() != want {
                continue;
            }
            if !st.followed_by.is_empty() {
                return Ok(Some(st.followed_by[0].clone()));
            }
            if is_metadata_status(&st) {
                metadata_gid = Some(gid);
                continue;
            }
            content.push((gid, parse_i64(&st.total_length)));
        }
        content.sort_by_key(|(_, total)| std::cmp::Reverse(*total));
        Ok(content
            .first()
            .map(|(g, _)| g.clone())
            .or(metadata_gid))
    }

    pub async fn find_gid_by_infohash(&self, infohash: &str) -> Result<Option<String>> {
        self.find_best_gid_for_magnet(infohash).await
    }
}

pub fn is_seeder(st: &Aria2Status) -> bool {
    matches!(st.seeder.to_ascii_lowercase().as_str(), "true" | "1")
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

/// True when aria2 is fetching or finished fetching torrent metadata (not the files).
pub fn is_metadata_status(st: &Aria2Status) -> bool {
    st.files
        .first()
        .is_some_and(|f| f.path.contains("[METADATA]"))
}
