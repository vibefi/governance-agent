use anyhow::{Result, anyhow};

use crate::{
    config::NetworkConfig,
    decoder::{decode_proposal_log, proposal_created_topic0},
    rpc::JsonRpcClient,
    types::Proposal,
};

#[derive(Debug)]
pub struct ChainAdapter {
    rpc: JsonRpcClient,
    governor_address: String,
    dapp_registry_address: String,
    topic0: String,
}

impl ChainAdapter {
    pub fn new(network: &NetworkConfig) -> Self {
        Self {
            rpc: JsonRpcClient::new(network.rpc_url.clone()),
            governor_address: network.governor_address.clone(),
            dapp_registry_address: network.dapp_registry_address.clone(),
            topic0: proposal_created_topic0(),
        }
    }

    pub async fn health_check(&self) -> Result<u64> {
        self.rpc.chain_id().await
    }

    pub async fn latest_block(&self) -> Result<u64> {
        self.rpc.block_number().await
    }

    pub async fn fetch_proposals(&self, from_block: u64, to_block: u64) -> Result<Vec<Proposal>> {
        if self.governor_address.is_empty() {
            return Ok(Vec::new());
        }
        let logs = self
            .rpc
            .get_logs(&self.governor_address, &self.topic0, from_block, to_block)
            .await?;

        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            let proposal = decode_proposal_log(&log, &self.dapp_registry_address)?;
            out.push(proposal);
        }
        Ok(out)
    }

    pub async fn fetch_proposal_by_id(
        &self,
        proposal_id: u64,
        from_block: u64,
    ) -> Result<Proposal> {
        let latest = self.latest_block().await?;
        let proposals = self.fetch_proposals(from_block, latest).await?;

        proposals
            .into_iter()
            .find(|p| p.proposal_id == proposal_id)
            .ok_or_else(|| anyhow!("proposal {proposal_id} not found"))
    }
}
