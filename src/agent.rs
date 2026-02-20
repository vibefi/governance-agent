use std::{fs, time::Duration};

use anyhow::Result;

use crate::{
    chain::ChainAdapter,
    config::AppConfig,
    decision::decide,
    ipfs::BundleFetcher,
    llm::CompositeLlm,
    notifier::MultiNotifier,
    review::review_proposal,
    signer::{DryRunVoteExecutor, KeystoreVoteExecutor, VoteExecutor, signing_readiness_reason},
    storage::{State, Storage},
    types::ProcessedProposal,
};

pub struct Agent {
    config: AppConfig,
    chain: ChainAdapter,
    storage: Storage,
    bundle_fetcher: BundleFetcher,
    llm: CompositeLlm,
    notifier: MultiNotifier,
    prompt_override: Option<String>,
}

impl Agent {
    pub fn new(config: AppConfig) -> Result<Self> {
        let prompt_override = config
            .review
            .prompt_file
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok());

        Ok(Self {
            chain: ChainAdapter::new(&config.network),
            storage: Storage::new(&config.storage)?,
            bundle_fetcher: BundleFetcher::new(&config.ipfs)?,
            llm: CompositeLlm::from_config(&config.llm),
            notifier: MultiNotifier::from_config(&config.notifications),
            config,
            prompt_override,
        })
    }

    pub async fn run_loop(&self, once: bool) -> Result<()> {
        tracing::debug!(
            config = %self.redacted_config_json(),
            "resolved run config"
        );
        tracing::info!(
            poll_interval_secs = self.config.poll_interval_secs,
            mode = if once { "single-pass" } else { "continuous" },
            state_path = %self.storage.state_path().display(),
            from_block = self.config.network.from_block,
            auto_vote = self.config.auto_vote,
            "agent run loop started"
        );
        if self.config.auto_vote {
            if let Some(reason) = signing_readiness_reason(&self.config.signer) {
                tracing::warn!(
                    reason = %reason,
                    "auto-vote enabled but signer is not fully configured; agent cannot submit votes"
                );
            }
        } else {
            tracing::info!(
                "auto-vote disabled; agent will run in dry-run mode and cannot submit votes"
            );
        }

        loop {
            self.scan_and_process_once().await?;
            if once {
                tracing::info!("agent run loop finished single pass");
                return Ok(());
            }
            tracing::info!(
                sleep_secs = self.config.poll_interval_secs,
                "scan cycle complete; waiting before next block check"
            );
            tokio::time::sleep(Duration::from_secs(self.config.poll_interval_secs)).await;
        }
    }

    pub async fn backfill(&self, from_block: u64, to_block: Option<u64>) -> Result<()> {
        let mut state = self.storage.load()?;
        let latest = self.chain.latest_block().await?;
        let end = to_block.unwrap_or(latest);
        self.process_range(&mut state, from_block, end).await?;
        state.last_scanned_block = state.last_scanned_block.max(end);
        self.storage.save(&state)?;
        Ok(())
    }

    pub async fn review_once(&self, proposal_id: u64) -> Result<()> {
        let proposal = self
            .chain
            .fetch_proposal_by_id(proposal_id, self.config.network.from_block)
            .await?;

        let review = review_proposal(
            &proposal,
            &self.config.review,
            &self.bundle_fetcher,
            &self.llm,
            self.prompt_override.as_deref(),
        )
        .await?;

        let decision = decide(&self.config.decision, &review);
        tracing::info!(
            proposal_id = proposal_id,
            vote = ?decision.vote,
            confidence = decision.confidence,
            "review-once complete"
        );

        Ok(())
    }

    pub async fn status(&self) -> Result<()> {
        let chain_id = self.chain.health_check().await?;
        tracing::info!(chain_id, "rpc health check succeeded");
        tracing::info!(
            transport = self.chain.transport().as_str(),
            "rpc transport mode"
        );

        let storage_path = self.storage.state_path().display().to_string();
        tracing::info!(path = storage_path, "storage path configured");
        let state = self.storage.load()?;
        tracing::info!(
            last_scanned_block = state.last_scanned_block,
            stored_proposals = state.proposals.len(),
            configured_from_block = self.config.network.from_block,
            "local scan state"
        );

        if self.config.notifications.telegram.enabled {
            tracing::info!("telegram notifier enabled");
        }

        Ok(())
    }

    async fn scan_and_process_once(&self) -> Result<()> {
        let mut state = self.storage.load()?;
        tracing::info!(
            last_scanned_block = state.last_scanned_block,
            stored_proposals = state.proposals.len(),
            configured_from_block = self.config.network.from_block,
            "loaded local scan state"
        );

        let latest = self.chain.latest_block().await?;
        if state.last_scanned_block > latest {
            tracing::warn!(
                last_scanned_block = state.last_scanned_block,
                latest_block = latest,
                stored_proposals = state.proposals.len(),
                "state cursor is ahead of chain tip; assuming chain reset and resetting local state"
            );
            state = State::default();
        }

        let (start, resume_source) = if state.last_scanned_block == 0 {
            (self.config.network.from_block, "config.from_block")
        } else {
            (
                state.last_scanned_block.saturating_add(1),
                "state.last_scanned_block+1",
            )
        };

        tracing::info!(
            start_block = start,
            latest_block = latest,
            resume_source,
            "checking chain for new blocks"
        );

        if latest < start {
            tracing::info!(
                start_block = start,
                latest_block = latest,
                "no new blocks to scan"
            );
            return Ok(());
        }

        self.process_range(&mut state, start, latest).await?;
        state.last_scanned_block = latest;
        self.storage.save(&state)?;

        Ok(())
    }

    async fn process_range(&self, state: &mut State, from_block: u64, to_block: u64) -> Result<()> {
        let proposals = self.chain.fetch_proposals(from_block, to_block).await?;
        if proposals.is_empty() {
            tracing::info!(from_block, to_block, "no proposals found in range");
            return Ok(());
        }

        tracing::info!(
            count = proposals.len(),
            from_block,
            to_block,
            "processing proposals"
        );

        let vote_executor: Box<dyn VoteExecutor> = if self.config.auto_vote {
            if let Some(reason) = signing_readiness_reason(&self.config.signer) {
                tracing::warn!(
                    reason = %reason,
                    "signer is not fully configured; continuing in dry-run mode (cannot vote)"
                );
                Box::new(DryRunVoteExecutor)
            } else {
                match KeystoreVoteExecutor::from_config(&self.config.network, &self.config.signer)
                    .await
                {
                    Ok(executor) => Box::new(executor),
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to initialize signer executor; continuing in dry-run mode (cannot vote)"
                        );
                        Box::new(DryRunVoteExecutor)
                    }
                }
            }
        } else {
            Box::new(DryRunVoteExecutor)
        };

        for proposal in proposals {
            let key = proposal.proposal_id.to_string();
            if state.proposals.contains_key(&key) {
                continue;
            }

            let review = review_proposal(
                &proposal,
                &self.config.review,
                &self.bundle_fetcher,
                &self.llm,
                self.prompt_override.as_deref(),
            )
            .await?;

            let decision = decide(&self.config.decision, &review);
            let vote_execution = match vote_executor.submit_vote(&proposal, &decision).await {
                Ok(vote) => Some(vote),
                Err(err) => {
                    tracing::warn!(proposal_id = proposal.proposal_id, error = %err, "vote submission failed");
                    None
                }
            };

            let processed = ProcessedProposal {
                proposal,
                review,
                decision,
                vote_execution,
            };

            self.notifier
                .notify_all(&format!(
                    "governance-agent processed proposal {} with vote {:?}",
                    processed.proposal.proposal_id, processed.decision.vote
                ))
                .await;

            state.proposals.insert(key, processed);
        }

        Ok(())
    }

    fn redacted_config_json(&self) -> String {
        let mut config = self.config.clone();
        if config.signer.keystore_password.is_some() {
            config.signer.keystore_password = Some("[REDACTED]".to_string());
        }

        serde_json::to_string_pretty(&config)
            .unwrap_or_else(|_| "<failed to serialize config>".to_string())
    }
}
