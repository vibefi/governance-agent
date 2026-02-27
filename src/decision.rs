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

    let (vote, mut reasons, requires_human_override) = if has_critical {
        (
            VoteChoice::Against,
            vec!["critical finding detected in proposal review".to_string()],
            false,
        )
    } else if review.score >= approve_min {
        (
            VoteChoice::For,
            vec![format!(
                "review score {:.2} is above {:.2} approval threshold",
                review.score, approve_min
            )],
            false,
        )
    } else if review.score <= reject_max {
        (
            VoteChoice::Against,
            vec![format!(
                "review score {:.2} is below {:.2} reject threshold",
                review.score, reject_max
            )],
            false,
        )
    } else {
        (
            VoteChoice::Abstain,
            vec![format!(
                "review score {:.2} is in abstain band [{:.2}, {:.2}]",
                review.score, reject_max, approve_min
            )],
            true,
        )
    };

    if let Some(score) = review.llm_score {
        reasons.push(format!("llm score: {:.2}", score));
    }
    let deterministic_weight = review.deterministic_weight.unwrap_or(0.70);
    let llm_weight = review.llm_weight.unwrap_or(0.30);
    let deterministic_score = review.deterministic_score.unwrap_or(review.score);
    match review.llm_score {
        Some(llm_score) => reasons.push(format!(
            "blended score = {:.2}*{:.2} + {:.2}*{:.2} = {:.2}",
            deterministic_weight, deterministic_score, llm_weight, llm_score, review.score
        )),
        None => reasons.push(format!(
            "llm score unavailable; using deterministic score {:.2}",
            deterministic_score
        )),
    }
    reasons.push(format!(
        "decision thresholds: reject <= {:.2}, approve >= {:.2}",
        reject_max, approve_min
    ));

    Decision {
        proposal_id: review.proposal_id.clone(),
        vote,
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
            proposal_id: "1".to_string(),
            root_cid: Some("bafy...".to_string()),
            findings,
            deterministic_score: Some(score),
            deterministic_weight: Some(0.70),
            llm_weight: Some(0.30),
            llm_score: None,
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
            deterministic_weight: Some(0.70),
            llm_weight: Some(0.30),
        }
    }

    #[test]
    fn conservative_approves_at_point_eight() {
        let decision = decide(&conservative_cfg(), &review(0.8, vec![]));
        assert_eq!(decision.vote, VoteChoice::For);
    }

    #[test]
    fn critical_finding_forces_against() {
        let decision = decide(
            &DecisionConfig {
                profile: Some(ConfidenceProfile::Aggressive),
                approve_threshold: None,
                reject_threshold: None,
                deterministic_weight: Some(0.70),
                llm_weight: Some(0.30),
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
        assert_eq!(decision.blocking_findings, vec!["bad".to_string()]);
    }

    #[test]
    fn low_blended_score_drives_against_under_default_thresholds() {
        let decision = decide(
            &conservative_cfg(),
            &ReviewResult {
                proposal_id: "1".to_string(),
                root_cid: Some("bafy...".to_string()),
                findings: vec![],
                deterministic_score: Some(0.20),
                deterministic_weight: Some(0.70),
                llm_weight: Some(0.30),
                llm_score: Some(0.05),
                llm_audit: None,
                score: 0.155,
                reviewed_at: Utc::now(),
            },
        );
        assert_eq!(decision.vote, VoteChoice::Against);
    }

    #[test]
    fn higher_llm_weight_can_shift_same_deterministic_score_outcome() {
        let default_weighted = ReviewResult {
            proposal_id: "1".to_string(),
            root_cid: Some("bafy...".to_string()),
            findings: vec![],
            deterministic_score: Some(0.90),
            deterministic_weight: Some(0.70),
            llm_weight: Some(0.30),
            llm_score: Some(0.20),
            llm_audit: None,
            score: 0.69,
            reviewed_at: Utc::now(),
        };
        let llm_heavy = ReviewResult {
            deterministic_weight: Some(0.10),
            llm_weight: Some(0.90),
            score: 0.27,
            ..default_weighted.clone()
        };

        let decision_default = decide(&conservative_cfg(), &default_weighted);
        let decision_llm_heavy = decide(&conservative_cfg(), &llm_heavy);

        assert_eq!(decision_default.vote, VoteChoice::Abstain);
        assert_eq!(decision_llm_heavy.vote, VoteChoice::Against);
    }
}
