use std::env;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    config::{LlmConfig, ProviderConfig},
    observability,
};

static REDACTION_PATTERNS: Lazy<[Regex; 5]> = Lazy::new(|| {
    [
        Regex::new(r"sk-[A-Za-z0-9_-]{16,}").expect("valid regex"),
        Regex::new(r"sk-ant-[A-Za-z0-9_-]{16,}").expect("valid regex"),
        Regex::new(r"(?i)bearer\s+[A-Za-z0-9._-]{16,}").expect("valid regex"),
        Regex::new(r"(?i)(api[_-]?key\s*[:=]\s*)([A-Za-z0-9._-]{8,})").expect("valid regex"),
        Regex::new(
            r"(?i)((?:eth(?:ereum)?[_-]?)?private[_-]?key\s*[:=]\s*)(0x[a-f0-9]{64}|[a-f0-9]{64})",
        )
        .expect("valid regex"),
    ]
});

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
            Box::new(OllamaProvider::new(&config.ollama)),
            Box::new(OpenAiLikeProvider::new("openai", &config.openai)),
            Box::new(VeniceProvider::new(&config.venice)),
            Box::new(AnthropicProvider::new(&config.anthropic)),
        ];
        Self { providers }
    }

    pub async fn analyze_best_effort(&self, ctx: &LlmContext) -> Option<LlmResponse> {
        let llm_started = observability::now();
        for provider in &self.providers {
            match provider.analyze(ctx).await {
                Ok(response) => {
                    observability::observe_stage_latency("llm_review", llm_started);
                    return Some(response);
                }
                Err(err) => {
                    observability::record_provider_error("llm", "provider_attempt");
                    tracing::warn!(error = %err, "llm provider attempt failed; trying next provider");
                    continue;
                }
            }
        }
        observability::observe_stage_latency("llm_review", llm_started);
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

        let text = self
            .call_responses_api(&base_url, &api_key, &model, &ctx.prompt)
            .await?;

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

impl OpenAiLikeProvider {
    async fn call_responses_api(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String> {
        let response = self
            .http
            .post(format!("{}/responses", base_url.trim_end_matches('/')))
            .bearer_auth(api_key)
            .json(&json!({
                "model": model,
                "input": prompt,
            }))
            .send()
            .await?;

        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "{} provider returned HTTP {} with body {}",
                self.name,
                status,
                body
            ));
        }

        extract_responses_text(&body)
            .ok_or_else(|| anyhow!("{} provider response missing output text", self.name))
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

struct OllamaProvider {
    cfg: ProviderConfig,
    http: Client,
}

impl OllamaProvider {
    fn new(cfg: &ProviderConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            http: Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn analyze(&self, ctx: &LlmContext) -> Result<LlmResponse> {
        if !self.cfg.enabled {
            return Err(anyhow!("provider disabled"));
        }

        let base_url = self
            .cfg
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("provider missing base_url"))?;
        let model = self
            .cfg
            .model
            .clone()
            .unwrap_or_else(|| "qwen3.5:9b".to_string());
        let api_key = match self.cfg.api_key_env.as_deref() {
            Some(key_var) => Some(
                env::var(key_var)
                    .map_err(|_| anyhow!("provider api key env var {key_var} is not set"))?,
            ),
            None => None,
        };

        let mut request = self
            .http
            .post(format!("{}/api/generate", base_url.trim_end_matches('/')))
            .json(&json!({
                "model": model,
                "prompt": ctx.prompt,
                "stream": false,
                "options": {
                    "temperature": 0.1
                }
            }));
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;

        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "ollama provider returned HTTP {} with body {}",
                status,
                body
            ));
        }

        let text = extract_ollama_text(&body)
            .ok_or_else(|| anyhow!("ollama provider response missing content"))?;

        Ok(LlmResponse {
            provider: "ollama".to_string(),
            model,
            text,
        })
    }
}

struct VeniceProvider {
    cfg: ProviderConfig,
    http: Client,
}

impl VeniceProvider {
    fn new(cfg: &ProviderConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            http: Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for VeniceProvider {
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
            .unwrap_or_else(|| "venice-uncensored".to_string());

        let response = self
            .http
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .bearer_auth(api_key)
            .json(&json!({
                "model": model,
                "temperature": 0.1,
                "messages": [
                    {"role": "user", "content": ctx.prompt}
                ]
            }))
            .send()
            .await?;

        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "venice provider returned HTTP {} with body {}",
                status,
                body
            ));
        }

        let text = extract_chat_completion_text(&body)
            .ok_or_else(|| anyhow!("venice provider response missing content"))?;

        Ok(LlmResponse {
            provider: "venice".to_string(),
            model,
            text,
        })
    }
}

pub fn redact_secrets(input: &str) -> String {
    let mut redacted = input.to_string();
    for regex in REDACTION_PATTERNS.iter() {
        redacted = regex
            .replace_all(&redacted, |caps: &regex::Captures<'_>| {
                if caps.len() > 2 {
                    format!("{}[REDACTED]", &caps[1])
                } else {
                    "[REDACTED]".to_string()
                }
            })
            .to_string();
    }

    redacted
}

fn extract_responses_text(body: &serde_json::Value) -> Option<String> {
    if let Some(text) = body.get("output_text").and_then(|value| value.as_str())
        && !text.trim().is_empty()
    {
        return Some(text.to_string());
    }

    let text = body
        .get("output")
        .and_then(|value| value.as_array())
        .and_then(|output| {
            output.iter().find_map(|item| {
                item.get("content")
                    .and_then(|value| value.as_array())
                    .and_then(|content| {
                        content.iter().find_map(|entry| {
                            entry
                                .get("text")
                                .and_then(|value| value.as_str())
                                .map(|value| value.to_string())
                        })
                    })
            })
        });

    text.filter(|value| !value.trim().is_empty())
}

fn extract_ollama_text(body: &serde_json::Value) -> Option<String> {
    if let Some(text) = body.get("response").and_then(|value| value.as_str())
        && !text.trim().is_empty()
    {
        return Some(text.to_string());
    }

    let text = body
        .get("message")
        .and_then(|value| value.get("content"))
        .and_then(|value| value.as_str());

    text.filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn extract_chat_completion_text(body: &serde_json::Value) -> Option<String> {
    let text = body
        .get("choices")
        .and_then(|value| value.as_array())
        .and_then(|choices| {
            choices.iter().find_map(|choice| {
                let content = choice
                    .get("message")
                    .and_then(|value| value.get("content"))?;
                if let Some(text) = content.as_str() {
                    return Some(text.to_string());
                }

                content.as_array().and_then(|parts| {
                    parts.iter().find_map(|part| {
                        part.get("text")
                            .and_then(|value| value.as_str())
                            .map(ToString::to_string)
                    })
                })
            })
        });

    text.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{extract_chat_completion_text, extract_ollama_text, redact_secrets};

    #[test]
    fn redacts_common_secret_patterns() {
        let text = "Authorization: Bearer sk-test-1234567890abcdef and api_key=abc123456789";
        let redacted = redact_secrets(text);
        assert!(!redacted.contains("sk-test-1234567890abcdef"));
        assert!(!redacted.contains("abc123456789"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_ethereum_private_key_patterns() {
        let key = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let text = format!("ethereum_private_key={key}");
        let redacted = redact_secrets(&text);
        assert!(!redacted.contains(key));
        assert!(redacted.contains("ethereum_private_key=[REDACTED]"));
    }

    #[test]
    fn extracts_ollama_generate_text() {
        let body = json!({ "response": "hello from ollama" });
        assert_eq!(
            extract_ollama_text(&body).as_deref(),
            Some("hello from ollama")
        );
    }

    #[test]
    fn extracts_ollama_chat_text() {
        let body = json!({ "message": { "content": "hello from chat" } });
        assert_eq!(
            extract_ollama_text(&body).as_deref(),
            Some("hello from chat")
        );
    }

    #[test]
    fn extracts_chat_completion_string_text() {
        let body = json!({
            "choices": [
                {
                    "message": {
                        "content": "hello from venice"
                    }
                }
            ]
        });
        assert_eq!(
            extract_chat_completion_text(&body).as_deref(),
            Some("hello from venice")
        );
    }

    #[test]
    fn extracts_chat_completion_array_text() {
        let body = json!({
            "choices": [
                {
                    "message": {
                        "content": [
                            {"type": "text", "text": "hello from structured content"}
                        ]
                    }
                }
            ]
        });
        assert_eq!(
            extract_chat_completion_text(&body).as_deref(),
            Some("hello from structured content")
        );
    }
}
