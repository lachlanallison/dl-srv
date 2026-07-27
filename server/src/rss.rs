use std::sync::Arc;

use anyhow::Result;
use regex::Regex;
use tracing::{error, info, warn};

use crate::manager::Manager;
use crate::store::{AddTaskInput, Store};

pub struct RssPoller {
    manager: Arc<Manager>,
    store: Arc<std::sync::Mutex<Store>>,
}

impl RssPoller {
    pub fn new(manager: Arc<Manager>, store: Arc<std::sync::Mutex<Store>>) -> Self {
        Self { manager, store }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                if let Err(e) = self.poll_all().await {
                    warn!(err = %e, "rss poll error");
                }
            }
        });
    }

    async fn poll_all(&self) -> Result<()> {
        let feeds = self.store.lock().unwrap().enabled_rss_feeds()?;
        for feed in feeds {
            if let Err(e) = self.poll_feed(&feed).await {
                error!(feed_id = %feed.id, err = %e, "rss feed poll failed");
            }
        }
        Ok(())
    }

    async fn poll_feed(&self, feed: &crate::store::RssFeed) -> Result<()> {
        let body = reqwest::get(&feed.url).await?.error_for_status()?.bytes().await?;
        let channel = rss::Channel::read_from(&body[..])?;
        let filter = feed
            .filter_regex
            .as_ref()
            .map(|p| Regex::new(p))
            .transpose()?;

        for item in channel.items() {
            let link = match item.link() {
                Some(l) if !l.is_empty() => l.to_string(),
                _ => continue,
            };

            if let Some(ref re) = filter {
                let title = item.title().unwrap_or("");
                if !re.is_match(title) && !re.is_match(&link) {
                    continue;
                }
            }

            let guid = item
                .guid()
                .map(|g| g.value().to_string())
                .unwrap_or_else(|| link.clone());

            if self.store.lock().unwrap().rss_seen(&feed.id, &guid)? {
                continue;
            }

            info!(feed_id = %feed.id, url = %link, "rss new item");
            self.store.lock().unwrap().mark_rss_seen(&feed.id, &guid)?;

            let input = AddTaskInput {
                url: link,
                category: feed.category.clone(),
                referer: Some(feed.url.clone()),
                cookies: None,
                force_ytdlp: false,
                quality: None,
                source: Some(format!("rss:{}", feed.id)),
            };
            if let Err(e) = self.manager.add_task(input).await {
                error!(feed_id = %feed.id, err = %e, "rss add_task failed");
            }
        }
        Ok(())
    }
}
