use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(deserialize_with = "deserialize_proposal_id")]
    pub proposal_id: String,
    pub proposer: String,
    pub description: String,
    pub vote_start: u64,
    pub vote_end: u64,
    pub block_number: u64,
    pub tx_hash: Option<String>,
    pub targets: Vec<String>,
    pub values: Vec<String>,
    pub calldatas: Vec<String>,
    pub action: DecodedAction,
    pub discovered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecodedAction {
    PublishDapp {
        root_cid: String,
        name: String,
        version: String,
        description: String,
    },
    UpgradeDapp {
        dapp_id: String,
        root_cid: String,
        name: String,
        version: String,
        description: String,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    #[serde(deserialize_with = "deserialize_proposal_id")]
    pub proposal_id: String,
    pub root_cid: Option<String>,
    pub findings: Vec<Finding>,
    pub llm_summary: Option<String>,
    pub llm_audit: Option<LlmAudit>,
    pub score: f32,
    pub reviewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmAudit {
    pub provider: String,
    pub model: String,
    pub prompt_redacted: String,
    pub response_redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    #[serde(deserialize_with = "deserialize_proposal_id")]
    pub proposal_id: String,
    pub vote: VoteChoice,
    pub confidence: f32,
    pub reasons: Vec<String>,
    pub blocking_findings: Vec<String>,
    pub requires_human_override: bool,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VoteChoice {
    For,
    Against,
    Abstain,
}

impl VoteChoice {
    pub fn to_support_u8(self) -> u8 {
        match self {
            VoteChoice::Against => 0,
            VoteChoice::For => 1,
            VoteChoice::Abstain => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteExecution {
    #[serde(deserialize_with = "deserialize_proposal_id")]
    pub proposal_id: String,
    pub submitted: bool,
    pub tx_hash: Option<String>,
    pub reason: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedProposal {
    pub proposal: Proposal,
    pub review: ReviewResult,
    pub decision: Decision,
    pub vote_execution: Option<VoteExecution>,
}

fn deserialize_proposal_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ProposalIdRepr {
        String(String),
        U64(u64),
    }

    match ProposalIdRepr::deserialize(deserializer)? {
        ProposalIdRepr::String(value) => Ok(value),
        ProposalIdRepr::U64(value) => Ok(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Holder {
        #[serde(deserialize_with = "super::deserialize_proposal_id")]
        proposal_id: String,
    }

    #[test]
    fn proposal_id_deserializes_from_string() {
        let value: Holder =
            serde_json::from_str(r#"{"proposal_id":"85353726111642088776893488059974230743342594789084151765762295675253395008791"}"#)
                .expect("string proposal id should parse");
        assert_eq!(
            value.proposal_id,
            "85353726111642088776893488059974230743342594789084151765762295675253395008791"
        );
    }

    #[test]
    fn proposal_id_deserializes_from_u64_for_backward_compat() {
        let value: Holder = serde_json::from_str(r#"{"proposal_id":123}"#)
            .expect("numeric proposal id should parse");
        assert_eq!(value.proposal_id, "123");
    }
}
