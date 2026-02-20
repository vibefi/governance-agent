use std::env;

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

#[async_trait]
impl VoteExecutor for KeystoreVoteExecutor {
    async fn submit_vote(&self, proposal: &Proposal, decision: &Decision) -> Result<VoteExecution> {
        let governor = IVfiGovernor::new(self.governor_address, self.provider.clone());
        let proposal_id = U256::from(decision.proposal_id);

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
        let _receipt = pending
            .get_receipt()
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

    if !decision.blocking_findings.is_empty() {
        text.push_str("; blockers=");
        text.push_str(&decision.blocking_findings.join(" || "));
    }

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
            blocking_findings: Vec::new(),
            requires_human_override: false,
            decided_at: Utc::now(),
        };

        let reason = build_vote_reason(&decision, 120);
        assert_eq!(reason.len(), 120);
    }
}
