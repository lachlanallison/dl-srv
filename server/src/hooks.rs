use reqwest::Client;
use tracing::{error, warn};

use crate::config::Config;
use crate::organize::{self, Organizer, ScanResult};
use crate::store::Task;

pub async fn on_task_completed(
    config: &Config,
    task: &Task,
    organizer: Option<&Organizer>,
) -> ScanResult {
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => Some(c),
        Err(e) => {
            error!(err = %e, "webhook client build failed");
            None
        }
    };

    if config.webhook_enabled {
        if let (Some(client), Some(url)) = (&client, &config.webhook_url) {
            match client.post(url).json(task).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    warn!(status = %resp.status(), url = %url, "webhook returned error status");
                }
                Err(e) => error!(err = %e, url = %url, "webhook request failed"),
                _ => {}
            }
        }
    }

    let result = if let Some(org) = organizer {
        org.organize_task(task).await
    } else {
        ScanResult::default()
    };

    let organiser_on = organize::is_active(config);
    let should_refresh = if organiser_on {
        result.moved > 0 && !result.dry_run
    } else {
        true
    };
    if should_refresh {
        if let Some(client) = &client {
            organize::refresh_jellyfin(client, config).await;
        }
    }

    result
}
