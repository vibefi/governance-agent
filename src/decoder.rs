use std::str::FromStr;

use alloy_primitives::{Address, keccak256};
use anyhow::{Result, anyhow};
use chrono::Utc;
use ethabi::{ParamType, Token};

use crate::{
    rpc::{RpcLog, parse_hex_bytes, parse_hex_u64},
    types::{DecodedAction, Proposal},
};

const PROPOSAL_CREATED_SIG: &str =
    "ProposalCreated(uint256,address,address[],uint256[],string[],bytes[],uint256,uint256,string)";
const PUBLISH_DAPP_SIG: &str = "publishDapp(bytes,string,string,string)";
const UPGRADE_DAPP_SIG: &str = "upgradeDapp(uint256,bytes,string,string,string)";

pub fn proposal_created_topic0() -> String {
    format!(
        "0x{}",
        hex::encode(keccak256(PROPOSAL_CREATED_SIG.as_bytes()))
    )
}

pub fn decode_proposal_log(log: &RpcLog, dapp_registry: &str) -> Result<Proposal> {
    let data = parse_hex_bytes(&log.data)?;
    let tokens = ethabi::decode(
        &[
            ParamType::Uint(256),
            ParamType::Address,
            ParamType::Array(Box::new(ParamType::Address)),
            ParamType::Array(Box::new(ParamType::Uint(256))),
            ParamType::Array(Box::new(ParamType::String)),
            ParamType::Array(Box::new(ParamType::Bytes)),
            ParamType::Uint(256),
            ParamType::Uint(256),
            ParamType::String,
        ],
        &data,
    )?;

    let proposal_id = as_u64(&tokens[0])?;
    let proposer = format_address(as_address(&tokens[1])?);
    let targets = as_address_vec(&tokens[2])?
        .into_iter()
        .map(format_address)
        .collect::<Vec<_>>();
    let values = as_u256_vec_to_string(&tokens[3])?;
    let calldatas = as_bytes_vec_hex(&tokens[5])?;
    let vote_start = as_u64(&tokens[6])?;
    let vote_end = as_u64(&tokens[7])?;
    let description = as_string(&tokens[8])?.to_string();

    let action = decode_action(&targets, &calldatas, dapp_registry);

    Ok(Proposal {
        proposal_id,
        proposer,
        description,
        vote_start,
        vote_end,
        block_number: log
            .block_number
            .as_deref()
            .map(parse_hex_u64)
            .transpose()?
            .unwrap_or_default(),
        tx_hash: log.tx_hash.clone(),
        targets,
        values,
        calldatas,
        action,
        discovered_at: Utc::now(),
    })
}

pub fn decode_action(
    targets: &[String],
    calldatas: &[String],
    dapp_registry: &str,
) -> DecodedAction {
    let normalized_registry = normalize_address_str(dapp_registry).unwrap_or_default();

    for (idx, target) in targets.iter().enumerate() {
        let Ok(normalized_target) = normalize_address_str(target) else {
            continue;
        };
        if normalized_target != normalized_registry {
            continue;
        }

        let Some(calldata_hex) = calldatas.get(idx) else {
            continue;
        };
        let Ok(calldata) = parse_hex_bytes(calldata_hex) else {
            continue;
        };

        if calldata.len() < 4 {
            continue;
        }

        let selector = &calldata[..4];
        let params = &calldata[4..];

        if selector == selector4(PUBLISH_DAPP_SIG).as_slice() {
            return decode_publish(params).unwrap_or_else(|err| DecodedAction::Unsupported {
                reason: format!("failed to decode publishDapp calldata: {err}"),
            });
        }

        if selector == selector4(UPGRADE_DAPP_SIG).as_slice() {
            return decode_upgrade(params).unwrap_or_else(|err| DecodedAction::Unsupported {
                reason: format!("failed to decode upgradeDapp calldata: {err}"),
            });
        }
    }

    DecodedAction::Unsupported {
        reason: "proposal has no recognized dapp publish/upgrade action".to_string(),
    }
}

fn decode_publish(params: &[u8]) -> Result<DecodedAction> {
    let tokens = ethabi::decode(
        &[
            ParamType::Bytes,
            ParamType::String,
            ParamType::String,
            ParamType::String,
        ],
        params,
    )?;

    let root_cid = decode_root_cid(as_bytes(&tokens[0])?);
    Ok(DecodedAction::PublishDapp {
        root_cid,
        name: as_string(&tokens[1])?.to_string(),
        version: as_string(&tokens[2])?.to_string(),
        description: as_string(&tokens[3])?.to_string(),
    })
}

fn decode_upgrade(params: &[u8]) -> Result<DecodedAction> {
    let tokens = ethabi::decode(
        &[
            ParamType::Uint(256),
            ParamType::Bytes,
            ParamType::String,
            ParamType::String,
            ParamType::String,
        ],
        params,
    )?;

    let dapp_id = as_u64(&tokens[0])?.to_string();
    let root_cid = decode_root_cid(as_bytes(&tokens[1])?);

    Ok(DecodedAction::UpgradeDapp {
        dapp_id,
        root_cid,
        name: as_string(&tokens[2])?.to_string(),
        version: as_string(&tokens[3])?.to_string(),
        description: as_string(&tokens[4])?.to_string(),
    })
}

pub fn decode_root_cid(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "".to_string();
    }

    match String::from_utf8(bytes.to_vec()) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => format!("0x{}", hex::encode(bytes)),
    }
}

fn selector4(signature: &str) -> [u8; 4] {
    let hash = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

fn normalize_address_str(input: &str) -> Result<String> {
    let addr = Address::from_str(input).map_err(|e| anyhow!(e.to_string()))?;
    Ok(format_address(addr))
}

fn format_address(addr: Address) -> String {
    format!("0x{}", hex::encode(addr))
}

fn as_u64(token: &Token) -> Result<u64> {
    if let Token::Uint(v) = token {
        return Ok(v.as_u64());
    }
    Err(anyhow!("expected uint token"))
}

fn as_address(token: &Token) -> Result<Address> {
    if let Token::Address(v) = token {
        let bytes: [u8; 20] = v.0;
        return Ok(Address::from_slice(&bytes));
    }
    Err(anyhow!("expected address token"))
}

fn as_address_vec(token: &Token) -> Result<Vec<Address>> {
    match token {
        Token::Array(items) => items.iter().map(as_address).collect(),
        _ => Err(anyhow!("expected address array")),
    }
}

fn as_u256_vec_to_string(token: &Token) -> Result<Vec<String>> {
    match token {
        Token::Array(items) => items
            .iter()
            .map(|item| match item {
                Token::Uint(v) => Ok(v.to_string()),
                _ => Err(anyhow!("expected uint in uint array")),
            })
            .collect(),
        _ => Err(anyhow!("expected uint array")),
    }
}

fn as_bytes_vec_hex(token: &Token) -> Result<Vec<String>> {
    match token {
        Token::Array(items) => items
            .iter()
            .map(|item| match item {
                Token::Bytes(bytes) => Ok(format!("0x{}", hex::encode(bytes))),
                _ => Err(anyhow!("expected bytes in bytes array")),
            })
            .collect(),
        _ => Err(anyhow!("expected bytes array")),
    }
}

fn as_bytes(token: &Token) -> Result<&[u8]> {
    if let Token::Bytes(bytes) = token {
        return Ok(bytes.as_slice());
    }
    Err(anyhow!("expected bytes token"))
}

fn as_string(token: &Token) -> Result<&str> {
    if let Token::String(value) = token {
        return Ok(value.as_str());
    }
    Err(anyhow!("expected string token"))
}

#[cfg(test)]
mod tests {
    use ethabi::Token;

    use super::{DecodedAction, decode_action, decode_root_cid, selector4};

    #[test]
    fn decode_root_cid_prefers_utf8() {
        let input = b"bafybeigdyrztv4";
        assert_eq!(decode_root_cid(input), "bafybeigdyrztv4");
    }

    #[test]
    fn decode_root_cid_falls_back_to_hex() {
        let input = [0xff, 0x01, 0x02];
        assert_eq!(decode_root_cid(&input), "0xff0102");
    }

    #[test]
    fn decode_publish_action_from_calldata() {
        let params = ethabi::encode(&[
            Token::Bytes(b"bafy123".to_vec()),
            Token::String("App".to_string()),
            Token::String("1.0.0".to_string()),
            Token::String("desc".to_string()),
        ]);
        let mut calldata = selector4("publishDapp(bytes,string,string,string)").to_vec();
        calldata.extend(params);

        let decoded = decode_action(
            &["0xfb84b57e757649dff3870f1381c67c9097d0c67f".to_string()],
            &[format!("0x{}", hex::encode(calldata))],
            "0xFb84B57E757649Dff3870F1381C67c9097D0c67f",
        );

        match decoded {
            DecodedAction::PublishDapp {
                root_cid,
                name,
                version,
                description,
            } => {
                assert_eq!(root_cid, "bafy123");
                assert_eq!(name, "App");
                assert_eq!(version, "1.0.0");
                assert_eq!(description, "desc");
            }
            _ => panic!("expected publish action"),
        }
    }

    #[test]
    fn decode_upgrade_action_from_calldata() {
        let params = ethabi::encode(&[
            Token::Uint(ethabi::ethereum_types::U256::from(42u64)),
            Token::Bytes(b"bafy-upgrade".to_vec()),
            Token::String("App".to_string()),
            Token::String("2.0.0".to_string()),
            Token::String("desc2".to_string()),
        ]);
        let mut calldata = selector4("upgradeDapp(uint256,bytes,string,string,string)").to_vec();
        calldata.extend(params);

        let decoded = decode_action(
            &["0xfb84b57e757649dff3870f1381c67c9097d0c67f".to_string()],
            &[format!("0x{}", hex::encode(calldata))],
            "0xFb84B57E757649Dff3870F1381C67c9097D0c67f",
        );

        match decoded {
            DecodedAction::UpgradeDapp {
                dapp_id,
                root_cid,
                name,
                version,
                description,
            } => {
                assert_eq!(dapp_id, "42");
                assert_eq!(root_cid, "bafy-upgrade");
                assert_eq!(name, "App");
                assert_eq!(version, "2.0.0");
                assert_eq!(description, "desc2");
            }
            _ => panic!("expected upgrade action"),
        }
    }
}
