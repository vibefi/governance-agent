use std::{env, sync::Arc};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use ethers::{
    contract::abigen,
    middleware::SignerMiddleware,
    providers::{Http, Provider},
    signers::{LocalWallet, Signer},
    types::{Address, U256},
};

use crate::{
    config::{NetworkConfig, SignerConfig},
    types::{Decision, VoteExecution},
};

abigen!(
    VfiGovernorContract,
    r#"[
        function state(uint256 proposalId) view returns (uint8)
        function hasVoted(uint256 proposalId, address account) view returns (bool)
        function castVoteWithReason(uint256 proposalId, uint8 support, string reason) returns (uint256)
    ]"#
);

#[async_trait]
pub trait VoteExecutor: Send + Sync {
    async fn submit_vote(&self, decision: &Decision) -> Result<VoteExecution>;
}

pub struct DryRunVoteExecutor;

#[async_trait]
impl VoteExecutor for DryRunVoteExecutor {
    async fn submit_vote(&self, decision: &Decision) -> Result<VoteExecution> {
        Ok(VoteExecution {
            proposal_id: decision.proposal_id,
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
    governor: VfiGovernorContract<SignerMiddleware<Provider<Http>, LocalWallet>>,
    signer_address: Address,
    max_vote_reason_len: usize,
}

impl KeystoreVoteExecutor {
    pub async fn from_config(network: &NetworkConfig, signer: &SignerConfig) -> Result<Self> {
        let keystore_path = signer
            .keystore_path
            .as_ref()
            .ok_or_else(|| anyhow!("auto-vote requires signer.keystore_path"))?;

        let password = resolve_keystore_password(signer)?;

        let wallet = LocalWallet::decrypt_keystore(keystore_path, password)
            .with_context(|| format!("failed to decrypt keystore {}", keystore_path.display()))?
            .with_chain_id(network.chain_id);

        let provider = Provider::<Http>::try_from(network.rpc_url.as_str())
            .with_context(|| format!("invalid rpc url {}", network.rpc_url))?;

        let governor_address = network
            .governor_address
            .parse::<Address>()
            .with_context(|| {
                format!(
                    "invalid governor address configured: {}",
                    network.governor_address
                )
            })?;

        let client = SignerMiddleware::new(provider, wallet);
        let signer_address = client.address();
        let governor = VfiGovernorContract::new(governor_address, Arc::new(client));

        Ok(Self {
            governor,
            signer_address,
            max_vote_reason_len: signer.max_vote_reason_len,
        })
    }
}

#[async_trait]
impl VoteExecutor for KeystoreVoteExecutor {
    async fn submit_vote(&self, decision: &Decision) -> Result<VoteExecution> {
        let proposal_id = U256::from(decision.proposal_id);

        let state: u8 = self
            .governor
            .state(proposal_id)
            .call()
            .await
            .context("failed to read proposal state")?;
        if state != 1 {
            return Err(anyhow!(
                "proposal {} is not Active; current state={}",
                decision.proposal_id,
                state
            ));
        }

        let has_voted = self
            .governor
            .has_voted(proposal_id, self.signer_address)
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

        let reason = build_vote_reason(decision, self.max_vote_reason_len);

        let call = self.governor.cast_vote_with_reason(
            proposal_id,
            decision.vote.to_support_u8(),
            reason.clone(),
        );
        let pending = call
            .send()
            .await
            .context("failed to submit castVoteWithReason tx")?;

        let tx_hash = format!("{:#x}", pending.tx_hash());
        let _ = pending
            .await
            .context("failed waiting for vote tx receipt")?;

        Ok(VoteExecution {
            proposal_id: decision.proposal_id,
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

pub fn build_vote_reason(decision: &Decision, max_len: usize) -> String {
    let mut text = format!(
        "governance-agent vote={} confidence={:.2}; {}",
        decision.vote.to_support_u8(),
        decision.confidence,
        decision.reasons.join(" | ")
    );

    if text.len() > max_len {
        text.truncate(max_len);
    }

    text
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::types::{Decision, VoteChoice};

    use super::build_vote_reason;

    #[test]
    fn vote_reason_is_truncated() {
        let decision = Decision {
            proposal_id: 1,
            vote: VoteChoice::For,
            confidence: 0.9,
            reasons: vec!["x".repeat(400)],
            requires_human_override: false,
            decided_at: Utc::now(),
        };

        let reason = build_vote_reason(&decision, 120);
        assert_eq!(reason.len(), 120);
    }
}
