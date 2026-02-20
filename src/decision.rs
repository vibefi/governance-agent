use chrono::Utc;

use crate::{
    config::ConfidenceProfile,
    types::{Decision, ReviewResult, Severity, VoteChoice},
};

pub fn decide(profile: ConfidenceProfile, review: &ReviewResult) -> Decision {
    let (approve_min, reject_max) = thresholds(profile);

    let has_critical = review
        .findings
        .iter()
        .any(|finding| finding.severity == Severity::Critical);

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
        requires_human_override,
        decided_at: Utc::now(),
    }
}

fn thresholds(profile: ConfidenceProfile) -> (f32, f32) {
    match profile {
        ConfidenceProfile::Conservative => (0.90, 0.30),
        ConfidenceProfile::Balanced => (0.75, 0.25),
        ConfidenceProfile::Aggressive => (0.60, 0.20),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        config::ConfidenceProfile,
        types::{Finding, ReviewResult, Severity, VoteChoice},
    };

    use super::decide;

    fn review(score: f32, findings: Vec<Finding>) -> ReviewResult {
        ReviewResult {
            proposal_id: 1,
            root_cid: Some("bafy...".to_string()),
            findings,
            llm_summary: None,
            score,
            reviewed_at: Utc::now(),
        }
    }

    #[test]
    fn conservative_abstains_in_middle_band() {
        let decision = decide(ConfidenceProfile::Conservative, &review(0.8, vec![]));
        assert_eq!(decision.vote, VoteChoice::Abstain);
    }

    #[test]
    fn critical_finding_forces_against() {
        let decision = decide(
            ConfidenceProfile::Aggressive,
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
    }
}
