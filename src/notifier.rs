use std::env;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;

use crate::config::NotificationConfig;

#[async_trait]
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
    async fn notify(&self, message: &str) -> Result<()>;
}

pub struct MultiNotifier {
    notifiers: Vec<Box<dyn Notifier>>,
}

impl MultiNotifier {
    pub fn from_config(config: &NotificationConfig) -> Self {
        let mut notifiers: Vec<Box<dyn Notifier>> = vec![Box::new(LogNotifier {})];

        if config.telegram.enabled {
            notifiers.push(Box::new(TelegramNotifier {
                bot_token_env: config.telegram.bot_token_env.clone(),
                chat_id: config.telegram.chat_id.clone(),
                client: Client::new(),
            }));
        }

        Self { notifiers }
    }

    pub async fn notify_all(&self, message: &str) {
        for notifier in &self.notifiers {
            if let Err(err) = notifier.notify(message).await {
                tracing::warn!(
                    target = "notifier",
                    notifier = notifier.name(),
                    error = %err,
                    "notification attempt failed"
                );
            }
        }
    }
}

pub struct LogNotifier {}

#[async_trait]
impl Notifier for LogNotifier {
    fn name(&self) -> &'static str {
        "log"
    }

    async fn notify(&self, message: &str) -> Result<()> {
        tracing::info!(target = "notifier", "{}", message);
        Ok(())
    }
}

pub struct TelegramNotifier {
    bot_token_env: Option<String>,
    chat_id: Option<String>,
    client: Client,
}

#[async_trait]
impl Notifier for TelegramNotifier {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn notify(&self, message: &str) -> Result<()> {
        let env_name = self
            .bot_token_env
            .clone()
            .ok_or_else(|| anyhow!("telegram bot token env var is not configured"))?;
        let token = env::var(&env_name)
            .map_err(|_| anyhow!("telegram bot token env var {env_name} is not set"))?;
        let chat_id = self
            .chat_id
            .clone()
            .ok_or_else(|| anyhow!("telegram chat_id is not configured"))?;

        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": message,
                "disable_web_page_preview": true,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("telegram API returned HTTP {}", response.status()));
        }

        Ok(())
    }
}
