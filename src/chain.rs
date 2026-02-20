use std::str::FromStr;

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::Filter,
};
use anyhow::{Context, Result, anyhow};

use crate::{
    config::NetworkConfig,
    decoder::{decode_proposal_log, proposal_created_topic0},
    types::Proposal,
};

#[derive(Debug)]
pub struct ChainAdapter {
    rpc_url: String,
    governor_address: Option<Address>,
    dapp_registry_address: String,
    topic0: String,
    transport: TransportKind,
}

#[derive(Debug, Clone, Copy)]
pub enum TransportKind {
    Http,
    Ws,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Ws => "ws",
        }
    }
}

impl ChainAdapter {
    pub fn new(network: &NetworkConfig) -> Self {
        let governor_address = Address::from_str(&network.governor_address).ok();
        let transport = if is_ws_url(&network.rpc_url) {
            TransportKind::Ws
        } else {
            TransportKind::Http
        };

        Self {
            rpc_url: network.rpc_url.clone(),
            governor_address,
            dapp_registry_address: network.dapp_registry_address.clone(),
            topic0: proposal_created_topic0(),
            transport,
        }
    }

    pub fn transport(&self) -> TransportKind {
        self.transport
    }

    pub async fn health_check(&self) -> Result<u64> {
        let provider = self.provider().await?;
        provider
            .get_chain_id()
            .await
            .context("failed to read chain id")
    }

    pub async fn latest_block(&self) -> Result<u64> {
        let provider = self.provider().await?;
        provider
            .get_block_number()
            .await
            .context("failed to read latest block")
    }

    pub async fn fetch_proposals(&self, from_block: u64, to_block: u64) -> Result<Vec<Proposal>> {
        let Some(governor) = self.governor_address else {
            return Ok(Vec::new());
        };

        let topic0 = self
            .topic0
            .parse::<alloy::primitives::B256>()
            .with_context(|| format!("invalid topic0 hash {}", self.topic0))?;

        let filter = Filter::new()
            .address(governor)
            .event_signature(topic0)
            .from_block(from_block)
            .to_block(to_block);

        let provider = self.provider().await?;
        let logs = provider.get_logs(&filter).await.with_context(|| {
            format!("failed to fetch ProposalCreated logs in range [{from_block}, {to_block}]")
        })?;

        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            let proposal = decode_proposal_log(&log, &self.dapp_registry_address)?;
            out.push(proposal);
        }

        Ok(out)
    }

    pub async fn fetch_proposal_by_id(
        &self,
        proposal_id: &str,
        from_block: u64,
    ) -> Result<Proposal> {
        let requested = parse_proposal_id(proposal_id)
            .with_context(|| format!("invalid proposal id {}", proposal_id))?;
        let latest = self.latest_block().await?;
        let proposals = self.fetch_proposals(from_block, latest).await?;

        for proposal in proposals {
            let Ok(candidate) = parse_proposal_id(&proposal.proposal_id) else {
                tracing::warn!(
                    proposal_id = %proposal.proposal_id,
                    "skipping proposal with unparsable proposal id"
                );
                continue;
            };

            if candidate == requested {
                return Ok(proposal);
            }
        }

        Err(anyhow!("proposal {proposal_id} not found"))
    }

    async fn provider(&self) -> Result<DynProvider> {
        ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .with_context(|| format!("failed to connect to rpc url {}", self.rpc_url))
            .map(|provider| provider.erased())
    }
}

fn is_ws_url(url: &str) -> bool {
    let trimmed = url.trim().to_ascii_lowercase();
    trimmed.starts_with("ws://") || trimmed.starts_with("wss://")
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

#[cfg(test)]
mod tests {
    use super::{is_ws_url, parse_proposal_id};

    #[test]
    fn ws_detection_works_for_ws_and_wss() {
        assert!(is_ws_url("ws://127.0.0.1:8546"));
        assert!(is_ws_url("wss://eth.example"));
        assert!(!is_ws_url("http://127.0.0.1:8545"));
        assert!(!is_ws_url("https://eth.example"));
    }

    #[test]
    fn proposal_id_parser_accepts_decimal_and_hex() {
        let decimal =
            "85353726111642088776893488059974230743342594789084151765762295675253395008791";

        let parsed_decimal = parse_proposal_id(decimal).expect("decimal parses");
        let hex = format!("{:#x}", parsed_decimal);
        let parsed_hex = parse_proposal_id(&hex).expect("hex parses");
        assert_eq!(parsed_decimal, parsed_hex);
    }
}
