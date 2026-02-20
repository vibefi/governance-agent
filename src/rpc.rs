use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[derive(Debug)]
pub struct JsonRpcClient {
    url: String,
    http: Client,
    id: AtomicU64,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RpcLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "transactionHash")]
    pub tx_hash: Option<String>,
}

impl JsonRpcClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            http: Client::new(),
            id: AtomicU64::new(1),
        }
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response = self
            .http
            .post(&self.url)
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("rpc request failed for method {method}"))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "rpc request returned HTTP {} for method {}",
                response.status(),
                method
            ));
        }

        let body: RpcResponse<T> = response
            .json()
            .await
            .with_context(|| format!("rpc response decode failed for method {method}"))?;

        if let Some(err) = body.error {
            return Err(anyhow!("rpc error {}: {}", err.code, err.message));
        }

        body.result
            .ok_or_else(|| anyhow!("rpc method {} missing result", method))
    }

    pub async fn block_number(&self) -> Result<u64> {
        let hex: String = self.call("eth_blockNumber", json!([])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn chain_id(&self) -> Result<u64> {
        let hex: String = self.call("eth_chainId", json!([])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn get_logs(
        &self,
        address: &str,
        topic0: &str,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<RpcLog>> {
        self.call(
            "eth_getLogs",
            json!([{
                "address": address,
                "fromBlock": format!("0x{:x}", from_block),
                "toBlock": format!("0x{:x}", to_block),
                "topics": [topic0]
            }]),
        )
        .await
    }
}

pub fn parse_hex_u64(value: &str) -> Result<u64> {
    let normalized = value.strip_prefix("0x").unwrap_or(value);
    u64::from_str_radix(normalized, 16).with_context(|| format!("invalid hex u64: {value}"))
}

pub fn parse_hex_bytes(value: &str) -> Result<Vec<u8>> {
    let normalized = value.strip_prefix("0x").unwrap_or(value);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(normalized).with_context(|| format!("invalid hex bytes: {value}"))
}
