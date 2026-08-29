use reqwest::Client;
use tracing::{error, warn};

use crate::config::Config;
use crate::store::Task;

pub async fn on_task_completed(config: &Config, task: &Task) {
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            error!(err = %e, "webhook client build failed");
            return;
        }
    };

    if config.webhook_enabled {
        if let Some(url) = &config.webhook_url {
            match client.post(url).json(task).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    warn!(status = %resp.status(), url = %url, "webhook returned error status");
                }
                Err(e) => error!(err = %e, url = %url, "webhook request failed"),
                _ => {}
            }
        }
    }

    if let Some(url) = &config.jellyfin_refresh_url {
        match client.get(url).send().await {
            Ok(resp) if !resp.status().is_success() => {
                warn!(status = %resp.status(), url = %url, "jellyfin refresh returned error status");
            }
            Err(e) => error!(err = %e, url = %url, "jellyfin refresh request failed"),
            _ => {}
        }
    }
}
