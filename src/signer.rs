use std::{env, str::FromStr};

use alloy::{
    network::EthereumWallet,
    primitives::{Address, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;

use crate::{
    config::{NetworkConfig, SignerConfig},
    types::{Decision, Proposal, VoteExecution},
};

sol! {
    #[sol(rpc)]
    interface IVfiGovernor {
        function state(uint256 proposalId) external view returns (uint8);
        function hasVoted(uint256 proposalId, address account) external view returns (bool);
        function castVoteWithReason(uint256 proposalId, uint8 support, string reason) external returns (uint256);
    }
}

const ACTIVE_PROPOSAL_STATE: u8 = 1;
const GWEI_IN_WEI: u128 = 1_000_000_000;

#[async_trait]
pub trait VoteExecutor: Send + Sync {
    async fn submit_vote(&self, proposal: &Proposal, decision: &Decision) -> Result<VoteExecution>;
}

pub struct DryRunVoteExecutor;

#[async_trait]
impl VoteExecutor for DryRunVoteExecutor {
    async fn submit_vote(
        &self,
        _proposal: &Proposal,
        decision: &Decision,
    ) -> Result<VoteExecution> {
        Ok(VoteExecution {
            proposal_id: decision.proposal_id.clone(),
            submitted: false,
            tx_hash: None,
            reason: format!(
                "dry-run: would submit support={} confidence={:.2}",
                decision.vote.to_support_u8(),
                decision.confidence
            ),
            at: Utc::now(),
        })
    }
}

pub struct KeystoreVoteExecutor {
    provider: DynProvider,
    governor_address: Address,
    signer_address: Address,
    max_vote_reason_len: usize,
    min_vote_blocks_remaining: u64,
    max_gas_price_gwei: Option<u64>,
    max_priority_fee_gwei: Option<u64>,
}

impl KeystoreVoteExecutor {
    pub async fn from_config(network: &NetworkConfig, signer: &SignerConfig) -> Result<Self> {
        let keystore_path = signer
            .keystore_path
            .as_ref()
            .ok_or_else(|| anyhow!("auto-vote requires signer.keystore_path"))?;

        let password = resolve_keystore_password(signer)?;

        let signer_key = PrivateKeySigner::decrypt_keystore(keystore_path, password)
            .with_context(|| format!("failed to decrypt keystore {}", keystore_path.display()))?;
        let signer_address = signer_key.address();

        let wallet = EthereumWallet::from(signer_key);
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect(&network.rpc_url)
            .await
            .with_context(|| format!("failed to connect to rpc url {}", network.rpc_url))?
            .erased();

        let governor_address = network
            .governor_address
            .parse::<Address>()
            .with_context(|| {
                format!(
                    "invalid governor address configured: {}",
                    network.governor_address
                )
            })?;

        Ok(Self {
            provider,
            governor_address,
            signer_address,
            max_vote_reason_len: signer.max_vote_reason_len,
            min_vote_blocks_remaining: signer.min_vote_blocks_remaining,
            max_gas_price_gwei: signer.max_gas_price_gwei,
            max_priority_fee_gwei: signer.max_priority_fee_gwei,
        })
    }
}

pub fn signing_readiness_reason(signer: &SignerConfig) -> Option<String> {
    let Some(keystore_path) = signer.keystore_path.as_ref() else {
        return Some("signer.keystore_path is not set".to_string());
    };
    if !keystore_path.exists() {
        return Some(format!(
            "signer.keystore_path does not exist: {}",
            keystore_path.display()
        ));
    }

    if let Some(password) = &signer.keystore_password
        && !password.trim().is_empty()
    {
        return None;
    }

    let env_name = signer
        .keystore_password_env
        .clone()
        .unwrap_or_else(|| "GOV_AGENT_KEYSTORE_PASSWORD".to_string());

    match env::var(&env_name) {
        Ok(password) if !password.trim().is_empty() => None,
        Ok(_) => Some(format!(
            "signer password env var {} is set but empty",
            env_name
        )),
        Err(_) => Some(format!(
            "signer password is missing: set signer.keystore_password or env {}",
            env_name
        )),
    }
}

#[async_trait]
impl VoteExecutor for KeystoreVoteExecutor {
    async fn submit_vote(&self, proposal: &Proposal, decision: &Decision) -> Result<VoteExecution> {
        let governor = IVfiGovernor::new(self.governor_address, self.provider.clone());
        let proposal_id = parse_proposal_id(&decision.proposal_id)?;

        let state = governor
            .state(proposal_id)
            .call()
            .await
            .context("failed to read proposal state")?;
        if state != ACTIVE_PROPOSAL_STATE {
            return Err(anyhow!(
                "proposal {} is not Active; current state={}",
                decision.proposal_id,
                state
            ));
        }

        let has_voted = governor
            .hasVoted(proposal_id, self.signer_address)
            .call()
            .await
            .context("failed to read hasVoted")?;
        if has_voted {
            return Err(anyhow!(
                "signer {} already voted on proposal {}",
                self.signer_address,
                decision.proposal_id
            ));
        }

        let latest_block = self
            .provider
            .get_block_number()
            .await
            .context("failed to read latest block before vote submit")?;
        let minimum_required = latest_block.saturating_add(self.min_vote_blocks_remaining);
        if proposal.vote_end <= minimum_required {
            return Err(anyhow!(
                "proposal {} is too close to deadline; vote_end={} latest_block={} min_remaining={}",
                proposal.proposal_id,
                proposal.vote_end,
                latest_block,
                self.min_vote_blocks_remaining
            ));
        }

        if let Some(max_gas_gwei) = self.max_gas_price_gwei {
            let gas_price = self
                .provider
                .get_gas_price()
                .await
                .context("failed to read gas price")?;
            let max_gas_wei = u128::from(max_gas_gwei).saturating_mul(GWEI_IN_WEI);
            if gas_price > max_gas_wei {
                return Err(anyhow!(
                    "gas price {} wei exceeds max configured {} gwei",
                    gas_price,
                    max_gas_gwei
                ));
            }
        }

        if let Some(max_priority_gwei) = self.max_priority_fee_gwei {
            let priority_fee = self
                .provider
                .get_max_priority_fee_per_gas()
                .await
                .context("failed to read max priority fee per gas")?;
            let max_priority_wei = u128::from(max_priority_gwei).saturating_mul(GWEI_IN_WEI);
            if priority_fee > max_priority_wei {
                return Err(anyhow!(
                    "priority fee {} wei exceeds max configured {} gwei",
                    priority_fee,
                    max_priority_gwei
                ));
            }
        }

        let reason = build_vote_reason(decision, self.max_vote_reason_len);
        let pending = governor
            .castVoteWithReason(proposal_id, decision.vote.to_support_u8(), reason.clone())
            .send()
            .await
            .context("failed to submit castVoteWithReason tx")?;

        let tx_hash = format!("{:#x}", pending.tx_hash());
        let receipt = pending
            .get_receipt()
            .await
            .context("failed waiting for vote tx receipt")?;
        if !receipt.status() {
            return Err(anyhow!(
                "vote tx {} reverted on-chain",
                tx_hash
            ));
        }

        Ok(VoteExecution {
            proposal_id: decision.proposal_id.clone(),
            submitted: true,
            tx_hash: Some(tx_hash),
            reason,
            at: Utc::now(),
        })
    }
}

fn resolve_keystore_password(signer: &SignerConfig) -> Result<String> {
    if let Some(value) = &signer.keystore_password {
        return Ok(value.clone());
    }

    let env_name = signer
        .keystore_password_env
        .clone()
        .unwrap_or_else(|| "GOV_AGENT_KEYSTORE_PASSWORD".to_string());

    env::var(&env_name).with_context(|| {
        format!(
            "keystore password is not set; provide signer.keystore_password or env {}",
            env_name
        )
    })
}

fn parse_proposal_id(value: &str) -> Result<U256> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("proposal id is empty"));
    }

    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return U256::from_str_radix(hex, 16)
            .map_err(|err| anyhow!("invalid hex proposal id {}: {}", value, err));
    }

    U256::from_str(trimmed).map_err(|err| anyhow!("invalid decimal proposal id {}: {}", value, err))
}

pub fn build_vote_reason(decision: &Decision, max_len: usize) -> String {
    let mut text = format!(
        "governance-agent vote={} confidence={:.2}; {}",
        decision.vote.to_support_u8(),
        decision.confidence,
        decision.reasons.join(" | ")
    );

    if !decision.blocking_findings.is_empty() {
        text.push_str("; blockers=");
        text.push_str(&decision.blocking_findings.join(" || "));
    }

    if text.len() > max_len {
        if max_len == 0 {
            text.clear();
        } else {
            let mut idx = max_len.min(text.len());
            while idx > 0 && !text.is_char_boundary(idx) {
                idx -= 1;
            }
            text.truncate(idx);
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use chrono::Utc;

    use crate::{
        config::SignerConfig,
        types::{Decision, VoteChoice},
    };

    use super::{build_vote_reason, signing_readiness_reason};

    #[test]
    fn vote_reason_is_truncated() {
        let decision = Decision {
            proposal_id: "1".to_string(),
            vote: VoteChoice::For,
            confidence: 0.9,
            reasons: vec!["x".repeat(400)],
            blocking_findings: Vec::new(),
            requires_human_override: false,
            decided_at: Utc::now(),
        };

        let reason = build_vote_reason(&decision, 120);
        assert_eq!(reason.len(), 120);
    }

    #[test]
    fn vote_reason_truncation_handles_utf8_boundaries() {
        let decision = Decision {
            proposal_id: "1".to_string(),
            vote: VoteChoice::For,
            confidence: 0.9,
            reasons: vec!["🚀".repeat(64)],
            blocking_findings: Vec::new(),
            requires_human_override: false,
            decided_at: Utc::now(),
        };

        let reason = build_vote_reason(&decision, 121);
        assert!(reason.len() <= 121);
        assert!(reason.is_char_boundary(reason.len()));
    }

    #[test]
    fn signer_readiness_requires_keystore_path() {
        let signer = SignerConfig {
            keystore_path: None,
            keystore_password_env: Some("TEST_GOV_AGENT_PASSWORD_ENV".to_string()),
            keystore_password: None,
            max_vote_reason_len: 240,
            min_vote_blocks_remaining: 3,
            max_gas_price_gwei: Some(200),
            max_priority_fee_gwei: Some(5),
        };

        let reason = signing_readiness_reason(&signer);
        assert!(reason.is_some());
        assert!(
            reason
                .unwrap_or_default()
                .contains("signer.keystore_path is not set")
        );
    }

    #[test]
    fn signer_readiness_accepts_inline_password_when_keystore_exists() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gov-agent-test-keystore-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));

        fs::write(&path, b"{}").expect("write temp keystore");
        let signer = SignerConfig {
            keystore_path: Some(PathBuf::from(&path)),
            keystore_password_env: Some("TEST_GOV_AGENT_PASSWORD_ENV".to_string()),
            keystore_password: Some("password".to_string()),
            max_vote_reason_len: 240,
            min_vote_blocks_remaining: 3,
            max_gas_price_gwei: Some(200),
            max_priority_fee_gwei: Some(5),
        };

        let reason = signing_readiness_reason(&signer);
        let _ = fs::remove_file(&path);
        assert!(reason.is_none());
    }
}
