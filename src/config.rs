use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::Cli;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub profile: String,
    pub auto_vote: bool,
    pub poll_interval_secs: u64,
    pub network: NetworkConfig,
    pub signer: SignerConfig,
    pub ipfs: IpfsConfig,
    pub storage: StorageConfig,
    pub review: ReviewConfig,
    pub decision: DecisionConfig,
    pub llm: LlmConfig,
    pub notifications: NotificationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub governor_address: String,
    pub dapp_registry_address: String,
    pub from_block: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpfsConfig {
    pub gateway_url: String,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerConfig {
    pub keystore_path: Option<PathBuf>,
    pub keystore_password_env: Option<String>,
    pub keystore_password: Option<String>,
    pub max_vote_reason_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub state_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    pub prompt_file: Option<PathBuf>,
    pub max_bundle_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionConfig {
    pub profile: ConfidenceProfile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfidenceProfile {
    Conservative,
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub openai: ProviderConfig,
    pub anthropic: ProviderConfig,
    pub opencode: ProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    pub telegram: TelegramConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token_env: Option<String>,
    pub chat_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PartialAppConfig {
    profile: Option<String>,
    auto_vote: Option<bool>,
    poll_interval_secs: Option<u64>,
    network: Option<NetworkConfig>,
    signer: Option<SignerConfig>,
    ipfs: Option<IpfsConfig>,
    storage: Option<StorageConfig>,
    review: Option<ReviewConfig>,
    decision: Option<DecisionConfig>,
    llm: Option<LlmConfig>,
    notifications: Option<NotificationConfig>,
}

impl AppConfig {
    pub fn load(cli: &Cli) -> Result<Self> {
        let mut cfg = Self::for_profile(&cli.profile);

        if let Some(path) = &cli.config {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read config file {}", path.display()))?;
            let partial: PartialAppConfig = toml::from_str(&raw)
                .with_context(|| format!("failed to parse TOML config {}", path.display()))?;
            cfg.merge_partial(partial);
        }

        cfg.apply_env();
        cfg.apply_cli(cli);

        Ok(cfg)
    }

    pub fn for_profile(profile: &str) -> Self {
        let mut cfg = match profile {
            "sepolia" => Self::sepolia_defaults(),
            _ => Self::devnet_defaults(),
        };
        cfg.profile = profile.to_string();
        cfg
    }

    fn home_data_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".governance-agent")
    }

    fn devnet_defaults() -> Self {
        Self {
            profile: "devnet".to_string(),
            auto_vote: false,
            poll_interval_secs: 5,
            network: NetworkConfig {
                name: "devnet".to_string(),
                chain_id: 31337,
                rpc_url: "http://127.0.0.1:8545".to_string(),
                governor_address: "".to_string(),
                dapp_registry_address: "".to_string(),
                from_block: 0,
            },
            signer: SignerConfig::defaults(),
            ipfs: IpfsConfig {
                gateway_url: "http://127.0.0.1:8080".to_string(),
                request_timeout_secs: 20,
            },
            storage: StorageConfig {
                data_dir: Self::home_data_dir(),
                state_file: "state.json".to_string(),
            },
            review: ReviewConfig {
                prompt_file: None,
                max_bundle_bytes: 40 * 1024 * 1024,
            },
            decision: DecisionConfig {
                profile: ConfidenceProfile::Conservative,
            },
            llm: LlmConfig::defaults(),
            notifications: NotificationConfig::defaults(),
        }
    }

    fn sepolia_defaults() -> Self {
        Self {
            profile: "sepolia".to_string(),
            auto_vote: false,
            poll_interval_secs: 12,
            network: NetworkConfig {
                name: "sepolia".to_string(),
                chain_id: 11155111,
                rpc_url: "".to_string(),
                governor_address: "0x753d33e2E61F249c87e6D33c4e04b39731776297".to_string(),
                dapp_registry_address: "0xFb84B57E757649Dff3870F1381C67c9097D0c67f".to_string(),
                from_block: 10239268,
            },
            signer: SignerConfig::defaults(),
            ipfs: IpfsConfig {
                gateway_url: "https://ipfs.io".to_string(),
                request_timeout_secs: 30,
            },
            storage: StorageConfig {
                data_dir: Self::home_data_dir(),
                state_file: "state.json".to_string(),
            },
            review: ReviewConfig {
                prompt_file: None,
                max_bundle_bytes: 40 * 1024 * 1024,
            },
            decision: DecisionConfig {
                profile: ConfidenceProfile::Conservative,
            },
            llm: LlmConfig::defaults(),
            notifications: NotificationConfig::defaults(),
        }
    }

    fn merge_partial(&mut self, partial: PartialAppConfig) {
        if let Some(v) = partial.profile {
            self.profile = v;
        }
        if let Some(v) = partial.auto_vote {
            self.auto_vote = v;
        }
        if let Some(v) = partial.poll_interval_secs {
            self.poll_interval_secs = v;
        }
        if let Some(v) = partial.network {
            self.network = v;
        }
        if let Some(v) = partial.signer {
            self.signer = v;
        }
        if let Some(v) = partial.ipfs {
            self.ipfs = v;
        }
        if let Some(v) = partial.storage {
            self.storage = v;
        }
        if let Some(v) = partial.review {
            self.review = v;
        }
        if let Some(v) = partial.decision {
            self.decision = v;
        }
        if let Some(v) = partial.llm {
            self.llm = v;
        }
        if let Some(v) = partial.notifications {
            self.notifications = v;
        }
    }

    fn apply_env(&mut self) {
        if let Ok(v) = env::var("GOV_AGENT_PROFILE") {
            self.profile = v;
        }
        if let Ok(v) = env::var("GOV_AGENT_RPC_URL") {
            self.network.rpc_url = v;
        }
        if let Ok(v) = env::var("GOV_AGENT_GOVERNOR") {
            self.network.governor_address = v;
        }
        if let Ok(v) = env::var("GOV_AGENT_DAPP_REGISTRY") {
            self.network.dapp_registry_address = v;
        }
        if let Ok(v) = env::var("GOV_AGENT_KEYSTORE_PATH") {
            self.signer.keystore_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = env::var("GOV_AGENT_KEYSTORE_PASSWORD_ENV") {
            self.signer.keystore_password_env = Some(v);
        }
        if let Ok(v) = env::var("GOV_AGENT_KEYSTORE_PASSWORD") {
            self.signer.keystore_password = Some(v);
        }
        if let Ok(v) = env::var("GOV_AGENT_MAX_VOTE_REASON_LEN")
            && let Ok(parsed) = v.parse::<usize>()
        {
            self.signer.max_vote_reason_len = parsed;
        }
        if let Ok(v) = env::var("GOV_AGENT_AUTO_VOTE") {
            self.auto_vote = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES");
        }
        if let Ok(v) = env::var("GOV_AGENT_DATA_DIR") {
            self.storage.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = env::var("GOV_AGENT_POLL_INTERVAL_SECS") {
            if let Ok(parsed) = v.parse::<u64>() {
                self.poll_interval_secs = parsed;
            }
        }
    }

    fn apply_cli(&mut self, cli: &Cli) {
        if let Some(url) = &cli.rpc_url {
            self.network.rpc_url = url.clone();
        }
        if cli.auto_vote {
            self.auto_vote = true;
        }
    }
}

impl LlmConfig {
    fn defaults() -> Self {
        Self {
            openai: ProviderConfig {
                enabled: true,
                base_url: Some("https://api.openai.com/v1".to_string()),
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                model: Some("gpt-4.1-mini".to_string()),
            },
            anthropic: ProviderConfig {
                enabled: true,
                base_url: Some("https://api.anthropic.com/v1".to_string()),
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                model: Some("claude-3-5-sonnet-latest".to_string()),
            },
            opencode: ProviderConfig {
                enabled: true,
                base_url: Some("http://127.0.0.1:4096/v1".to_string()),
                api_key_env: Some("OPENCODE_API_KEY".to_string()),
                model: Some("default".to_string()),
            },
        }
    }
}

impl NotificationConfig {
    fn defaults() -> Self {
        Self {
            telegram: TelegramConfig {
                enabled: false,
                bot_token_env: Some("GOV_AGENT_TELEGRAM_BOT_TOKEN".to_string()),
                chat_id: None,
            },
        }
    }
}

impl SignerConfig {
    fn defaults() -> Self {
        Self {
            keystore_path: None,
            keystore_password_env: Some("GOV_AGENT_KEYSTORE_PASSWORD".to_string()),
            keystore_password: None,
            max_vote_reason_len: 240,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn sepolia_defaults_include_known_addresses() {
        let cfg = AppConfig::for_profile("sepolia");
        assert_eq!(cfg.network.chain_id, 11155111);
        assert_eq!(
            cfg.network.governor_address,
            "0x753d33e2E61F249c87e6D33c4e04b39731776297"
        );
        assert_eq!(
            cfg.network.dapp_registry_address,
            "0xFb84B57E757649Dff3870F1381C67c9097D0c67f"
        );
    }

    #[test]
    fn signer_defaults_are_safe_for_dry_run() {
        let cfg = AppConfig::for_profile("devnet");
        assert!(cfg.signer.keystore_path.is_none());
        assert_eq!(cfg.signer.max_vote_reason_len, 240);
        assert_eq!(
            cfg.signer.keystore_password_env.as_deref(),
            Some("GOV_AGENT_KEYSTORE_PASSWORD")
        );
    }
}
