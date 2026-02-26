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
const MAX_BUNDLE_INDEX_BYTES: usize = 64 * 1024;
const MAX_BUNDLE_CONTENT_BYTES: usize = 256 * 1024;
const MAX_BUNDLE_CONTENT_FETCHES: usize = 120;

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

    let bundle_snapshot = if let (Some(cid), Some(m)) = (&root_cid, manifest.as_ref()) {
        Some(
            build_bundle_snapshot(bundle_fetcher, cid, m)
                .await
                .unwrap_or_else(|err| format!("Bundle snapshot unavailable: {err}")),
        )
    } else {
        None
    };

    let llm_output = build_llm_summary(
        proposal,
        manifest.as_ref(),
        bundle_snapshot.as_deref(),
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
        proposal_id: proposal.proposal_id.clone(),
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
    bundle_snapshot: Option<&str>,
    llm: &CompositeLlm,
    prompt_override: Option<&str>,
    llm_context: &[String],
) -> Option<(String, LlmAudit)> {
    let prompt = match prompt_override {
        Some(custom) => format!(
            "{custom}\n\n{}",
            review_prompt(proposal, manifest, bundle_snapshot, llm_context)
        ),
        None => review_prompt(proposal, manifest, bundle_snapshot, llm_context),
    };

    let prompt_preview = preview_chars(&prompt, 500);
    tracing::debug!(
        proposal_id = %proposal.proposal_id,
        prompt_preview = %prompt_preview,
        prompt_len = prompt.len(),
        "review stage prepared LLM prompt preview"
    );
    tracing::trace!(
        proposal_id = %proposal.proposal_id,
        prompt = %prompt,
        "review stage prepared full LLM prompt"
    );

    llm.analyze_best_effort(&LlmContext {
        prompt: prompt.clone(),
    })
    .await
    .map(|resp| {
        let response_preview = preview_chars(&resp.text, 500);
        tracing::debug!(
            proposal_id = %proposal.proposal_id,
            provider = %resp.provider,
            model = %resp.model,
            response_preview = %response_preview,
            response_len = resp.text.len(),
            "review stage received LLM response preview"
        );
        tracing::trace!(
            proposal_id = %proposal.proposal_id,
            provider = %resp.provider,
            model = %resp.model,
            response = %resp.text,
            "review stage received full LLM response"
        );
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
    bundle_snapshot: Option<&str>,
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

    let bundle_section = bundle_snapshot.unwrap_or("Bundle snapshot unavailable.");

    format!(
        "Review this governance proposal and provide risk summary. Proposal id: {}. Description: {}. Action: {:?}. {}\n\nStatic analysis context:\n{}\n\nBundle snapshot:\n{}",
        proposal.proposal_id,
        proposal.description,
        proposal.action,
        manifest_snippet,
        context,
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

fn preview_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...[truncated]")
    } else {
        preview
    }
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
    use std::fs;

    use crate::{
        config::IpfsConfig,
        ipfs::{BundleFetcher, Manifest, ManifestFile},
    };

    use super::{
        build_bundle_snapshot, contains_suspicious_script_cmd, detect_suspicious_tokens,
        preview_chars,
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
    fn preview_chars_appends_truncated_marker_when_needed() {
        let preview = preview_chars("abcdef", 3);
        assert_eq!(preview, "abc...[truncated]");
    }

    #[test]
    fn preview_chars_returns_full_text_when_short() {
        let preview = preview_chars("abc", 10);
        assert_eq!(preview, "abc");
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
}
