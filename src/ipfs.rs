use std::time::Duration;

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
}

impl BundleFetcher {
    pub fn new(cfg: &IpfsConfig) -> Result<Self> {
        let gateway = cfg.gateway_url.trim_end_matches('/').to_string();
        let client = Client::builder()
            .timeout(Duration::from_secs(cfg.request_timeout_secs))
            .build()
            .context("failed to build http client for ipfs")?;

        Ok(Self { gateway, client })
    }

    pub async fn fetch_manifest(&self, root_cid: &str) -> Result<Manifest> {
        if root_cid.is_empty() {
            return Err(anyhow!("root CID is empty"));
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

        response
            .json::<Manifest>()
            .await
            .context("failed to decode manifest.json")
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

        match std::str::from_utf8(&bytes) {
            Ok(text) => Ok(Some(text.to_string())),
            Err(_) => Ok(None),
        }
    }
}
