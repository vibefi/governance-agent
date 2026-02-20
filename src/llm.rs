use std::env;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{LlmConfig, ProviderConfig};

#[derive(Debug, Clone)]
pub struct LlmContext {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub provider: String,
    pub model: String,
    pub text: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn analyze(&self, ctx: &LlmContext) -> Result<LlmResponse>;
}

pub struct CompositeLlm {
    providers: Vec<Box<dyn LlmProvider>>,
}

impl CompositeLlm {
    pub fn from_config(config: &LlmConfig) -> Self {
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(OpenAiLikeProvider::new("openai", &config.openai)),
            Box::new(AnthropicProvider::new(&config.anthropic)),
            Box::new(OpenAiLikeProvider::new("opencode", &config.opencode)),
        ];
        Self { providers }
    }

    pub async fn analyze_best_effort(&self, ctx: &LlmContext) -> Option<LlmResponse> {
        for provider in &self.providers {
            match provider.analyze(ctx).await {
                Ok(response) => return Some(response),
                Err(err) => {
                    tracing::warn!(error = %err, "llm provider attempt failed; trying next provider");
                    continue;
                }
            }
        }
        None
    }
}

struct OpenAiLikeProvider {
    name: String,
    cfg: ProviderConfig,
    http: Client,
}

impl OpenAiLikeProvider {
    fn new(name: &str, cfg: &ProviderConfig) -> Self {
        Self {
            name: name.to_string(),
            cfg: cfg.clone(),
            http: Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiLikeProvider {
    async fn analyze(&self, ctx: &LlmContext) -> Result<LlmResponse> {
        if !self.cfg.enabled {
            return Err(anyhow!("provider disabled"));
        }

        let key_var = self
            .cfg
            .api_key_env
            .clone()
            .ok_or_else(|| anyhow!("provider missing api_key_env"))?;
        let api_key = env::var(&key_var)
            .map_err(|_| anyhow!("provider api key env var {key_var} is not set"))?;

        let base_url = self
            .cfg
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("provider missing base_url"))?;
        let model = self
            .cfg
            .model
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let response = self
            .http
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .bearer_auth(api_key)
            .json(&json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": "You are a governance review assistant."},
                    {"role": "user", "content": ctx.prompt}
                ],
                "temperature": 0.1
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "{} provider returned HTTP {}",
                self.name,
                response.status()
            ));
        }

        let body: serde_json::Value = response.json().await?;
        let text = body
            .get("choices")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        if text.is_empty() {
            return Err(anyhow!("{} provider response missing content", self.name));
        }

        Ok(LlmResponse {
            provider: self.name.clone(),
            model,
            text,
        })
    }
}

struct AnthropicProvider {
    cfg: ProviderConfig,
    http: Client,
}

impl AnthropicProvider {
    fn new(cfg: &ProviderConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            http: Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn analyze(&self, ctx: &LlmContext) -> Result<LlmResponse> {
        if !self.cfg.enabled {
            return Err(anyhow!("provider disabled"));
        }

        let key_var = self
            .cfg
            .api_key_env
            .clone()
            .ok_or_else(|| anyhow!("provider missing api_key_env"))?;
        let api_key = env::var(&key_var)
            .map_err(|_| anyhow!("provider api key env var {key_var} is not set"))?;

        let base_url = self
            .cfg
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("provider missing base_url"))?;
        let model = self
            .cfg
            .model
            .clone()
            .unwrap_or_else(|| "claude-3-5-sonnet-latest".to_string());

        let response = self
            .http
            .post(format!("{}/messages", base_url.trim_end_matches('/')))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": model,
                "max_tokens": 1024,
                "temperature": 0.1,
                "messages": [
                    {"role": "user", "content": ctx.prompt}
                ]
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "anthropic provider returned HTTP {}",
                response.status()
            ));
        }

        let body: serde_json::Value = response.json().await?;
        let text = body
            .get("content")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        if text.is_empty() {
            return Err(anyhow!("anthropic provider response missing content"));
        }

        Ok(LlmResponse {
            provider: "anthropic".to_string(),
            model,
            text,
        })
    }
}
