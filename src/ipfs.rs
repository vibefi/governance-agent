use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::IpfsConfig;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub entry: Option<String>,
    pub files: Option<Vec<ManifestFile>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestFile {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct BundleFetcher {
    gateway: String,
    client: Client,
    cache_root: PathBuf,
}

impl BundleFetcher {
    pub fn new(cfg: &IpfsConfig) -> Result<Self> {
        let gateway = cfg.gateway_url.trim_end_matches('/').to_string();
        let client = Client::builder()
            .timeout(Duration::from_secs(cfg.request_timeout_secs))
            .build()
            .context("failed to build http client for ipfs")?;

        let cache_root = cfg
            .cache_dir
            .clone()
            .unwrap_or_else(default_shared_cache_dir);
        fs::create_dir_all(&cache_root)
            .with_context(|| format!("failed to create ipfs cache dir {}", cache_root.display()))?;

        Ok(Self {
            gateway,
            client,
            cache_root,
        })
    }

    pub async fn fetch_manifest(&self, root_cid: &str) -> Result<Manifest> {
        if root_cid.is_empty() {
            return Err(anyhow!("root CID is empty"));
        }

        if let Some(path) = self.cache_path(root_cid, "manifest.json")
            && path.exists()
        {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed reading cached manifest {}", path.display()))?;
            let manifest = serde_json::from_slice::<Manifest>(&bytes)
                .with_context(|| format!("failed decoding cached manifest {}", path.display()))?;
            return Ok(manifest);
        }

        let url = format!("{}/ipfs/{}/manifest.json", self.gateway, root_cid);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("ipfs gateway request failed")?;

        if !response.status().is_success() {
            return Err(anyhow!("ipfs gateway returned HTTP {}", response.status()));
        }

        let bytes = response
            .bytes()
            .await
            .context("failed reading manifest response bytes")?
            .to_vec();
        let manifest =
            serde_json::from_slice::<Manifest>(&bytes).context("failed to decode manifest.json")?;

        if let Some(path) = self.cache_path(root_cid, "manifest.json") {
            let _ = write_atomic(&path, &bytes);
        }

        Ok(manifest)
    }

    pub async fn fetch_text_file(
        &self,
        root_cid: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<String>> {
        if root_cid.is_empty() || path.is_empty() {
            return Ok(None);
        }

        if let Some(cache_path) = self.cache_path(root_cid, path)
            && cache_path.exists()
        {
            let bytes = fs::read(&cache_path)
                .with_context(|| format!("failed reading cached file {}", cache_path.display()))?;
            if bytes.len() > max_bytes {
                return Ok(None);
            }
            return Ok(std::str::from_utf8(&bytes)
                .ok()
                .map(|text| text.to_string()));
        }

        let url = format!("{}/ipfs/{}/{}", self.gateway, root_cid, path);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("ipfs gateway request failed")?;

        if !response.status().is_success() {
            return Ok(None);
        }

        if let Some(content_length) = response.content_length()
            && content_length > max_bytes as u64
        {
            return Ok(None);
        }

        let bytes = response.bytes().await?;
        if bytes.len() > max_bytes {
            return Ok(None);
        }

        let text = match std::str::from_utf8(&bytes) {
            Ok(value) => value.to_string(),
            Err(_) => return Ok(None),
        };

        if let Some(cache_path) = self.cache_path(root_cid, path) {
            let _ = write_atomic(&cache_path, bytes.as_ref());
        }

        Ok(Some(text))
    }

    fn cache_path(&self, root_cid: &str, relative: &str) -> Option<PathBuf> {
        if root_cid.is_empty() || root_cid.contains(['/', '\\']) {
            return None;
        }

        let mut out = self.cache_root.join(root_cid);
        let rel = safe_relative_path(relative)?;
        out.push(rel);
        Some(out)
    }
}

fn default_shared_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("VibeFi")
}

fn safe_relative_path(input: &str) -> Option<PathBuf> {
    let path = Path::new(input);
    if path.is_absolute() {
        return None;
    }

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(seg) => out.push(seg),
            _ => return None,
        }
    }

    if out.as_os_str().is_empty() {
        return None;
    }

    Some(out)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating cache dir {}", parent.display()))?;
    }

    let tmp_name = format!(
        "{}.tmp.{}.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache"),
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let tmp_path = path.with_file_name(tmp_name);

    fs::write(&tmp_path, bytes)
        .with_context(|| format!("failed writing temp cache file {}", tmp_path.display()))?;

    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(err) if path.exists() => {
            let _ = fs::remove_file(&tmp_path);
            Ok(())
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            Err(err).with_context(|| {
                format!(
                    "failed to move temp cache file {} to {}",
                    tmp_path.display(),
                    path.display()
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::safe_relative_path;

    #[test]
    fn relative_path_rejects_traversal() {
        assert!(safe_relative_path("../x").is_none());
        assert!(safe_relative_path("/absolute").is_none());
        assert!(safe_relative_path("ok/file.txt").is_some());
    }
}
