use chrono::Utc;

use crate::{
    config::DecisionConfig,
    types::{Decision, ReviewResult, Severity, VoteChoice},
};

pub fn decide(config: &DecisionConfig, review: &ReviewResult) -> Decision {
    let (approve_min, reject_max) = config.resolved_thresholds();

    let blocking_findings = review
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Critical)
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    let has_critical = !blocking_findings.is_empty();

    let (vote, confidence, mut reasons, requires_human_override) = if has_critical {
        (
            VoteChoice::Against,
            0.95,
            vec!["critical finding detected in proposal review".to_string()],
            false,
        )
    } else if review.score >= approve_min {
        (
            VoteChoice::For,
            review.score,
            vec![format!(
                "review score {:.2} is above {:.2} approval threshold",
                review.score, approve_min
            )],
            false,
        )
    } else if review.score <= reject_max {
        (
            VoteChoice::Against,
            1.0 - review.score,
            vec![format!(
                "review score {:.2} is below {:.2} reject threshold",
                review.score, reject_max
            )],
            false,
        )
    } else {
        (
            VoteChoice::Abstain,
            0.5,
            vec![format!(
                "review score {:.2} is in abstain band [{:.2}, {:.2}]",
                review.score, reject_max, approve_min
            )],
            true,
        )
    };

    if let Some(summary) = &review.llm_summary {
        reasons.push(format!("llm summary: {summary}"));
    }

    Decision {
        proposal_id: review.proposal_id,
        vote,
        confidence,
        reasons,
        blocking_findings,
        requires_human_override,
        decided_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        config::{ConfidenceProfile, DecisionConfig},
        types::{Finding, ReviewResult, Severity, VoteChoice},
    };

    use super::decide;

    fn review(score: f32, findings: Vec<Finding>) -> ReviewResult {
        ReviewResult {
            proposal_id: 1,
            root_cid: Some("bafy...".to_string()),
            findings,
            llm_summary: None,
            llm_audit: None,
            score,
            reviewed_at: Utc::now(),
        }
    }

    fn conservative_cfg() -> DecisionConfig {
        DecisionConfig {
            profile: Some(ConfidenceProfile::Conservative),
            approve_threshold: None,
            reject_threshold: None,
        }
    }

    #[test]
    fn conservative_abstains_in_middle_band() {
        let decision = decide(&conservative_cfg(), &review(0.8, vec![]));
        assert_eq!(decision.vote, VoteChoice::Abstain);
    }

    #[test]
    fn critical_finding_forces_against() {
        let decision = decide(
            &DecisionConfig {
                profile: Some(ConfidenceProfile::Aggressive),
                approve_threshold: None,
                reject_threshold: None,
            },
            &review(
                0.95,
                vec![Finding {
                    severity: Severity::Critical,
                    message: "bad".to_string(),
                }],
            ),
        );
        assert_eq!(decision.vote, VoteChoice::Against);
        assert!(decision.confidence > 0.9);
        assert_eq!(decision.blocking_findings, vec!["bad".to_string()]);
    }
}
