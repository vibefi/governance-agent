use std::str::FromStr;

use alloy::{
    primitives::{Address, U256},
    rpc::types::Log as RpcLog,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Result, anyhow};
use chrono::Utc;

use crate::types::{DecodedAction, Proposal};

sol! {
    event ProposalCreated(
        uint256 proposalId,
        address proposer,
        address[] targets,
        uint256[] values,
        string[] signatures,
        bytes[] calldatas,
        uint256 voteStart,
        uint256 voteEnd,
        string description
    );

    function publishDapp(bytes rootCid, string name, string version, string description);
    function upgradeDapp(uint256 dappId, bytes rootCid, string name, string version, string description);
}

pub fn proposal_created_topic0() -> String {
    format!("{:#x}", ProposalCreated::SIGNATURE_HASH)
}

pub fn decode_proposal_log(log: &RpcLog, dapp_registry: &str) -> Result<Proposal> {
    let decoded = log
        .log_decode_validate::<ProposalCreated>()
        .map_err(|err| anyhow!("failed to decode ProposalCreated log: {err}"))?;
    let event = decoded.inner.data;

    let proposal_id = event.proposalId.to_string();
    let vote_start = u256_to_u64(event.voteStart, "voteStart")?;
    let vote_end = u256_to_u64(event.voteEnd, "voteEnd")?;

    let targets = event
        .targets
        .into_iter()
        .map(|addr| format!("{:#x}", addr))
        .collect::<Vec<_>>();
    let values = event
        .values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let calldatas = event
        .calldatas
        .into_iter()
        .map(|data| format!("0x{}", hex::encode(data)))
        .collect::<Vec<_>>();

    let action = decode_action(&targets, &calldatas, dapp_registry);

    Ok(Proposal {
        proposal_id,
        proposer: format!("{:#x}", event.proposer),
        description: event.description,
        vote_start,
        vote_end,
        block_number: log.block_number.unwrap_or_default(),
        tx_hash: log.transaction_hash.map(|hash| format!("{:#x}", hash)),
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
    let Ok(dapp_registry_addr) = Address::from_str(dapp_registry) else {
        return DecodedAction::Unsupported {
            reason: format!("invalid dapp registry address configured: {dapp_registry}"),
        };
    };

    for (idx, target) in targets.iter().enumerate() {
        let Ok(target_addr) = Address::from_str(target) else {
            continue;
        };

        if target_addr != dapp_registry_addr {
            continue;
        }

        let Some(calldata_hex) = calldatas.get(idx) else {
            continue;
        };
        let Ok(calldata) = parse_calldata(calldata_hex) else {
            continue;
        };

        if let Ok(call) = publishDappCall::abi_decode(&calldata) {
            return DecodedAction::PublishDapp {
                root_cid: decode_root_cid(call.rootCid.as_ref()),
                name: call.name,
                version: call.version,
                description: call.description,
            };
        }

        if let Ok(call) = upgradeDappCall::abi_decode(&calldata) {
            return DecodedAction::UpgradeDapp {
                dapp_id: call.dappId.to_string(),
                root_cid: decode_root_cid(call.rootCid.as_ref()),
                name: call.name,
                version: call.version,
                description: call.description,
            };
        }

        return DecodedAction::Unsupported {
            reason: "target matches dapp registry but calldata did not decode as publishDapp or upgradeDapp"
                .to_string(),
        };
    }

    DecodedAction::Unsupported {
        reason: "proposal has no recognized dapp publish/upgrade action".to_string(),
    }
}

pub fn decode_root_cid(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    match String::from_utf8(bytes.to_vec()) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => format!("0x{}", hex::encode(bytes)),
    }
}

fn u256_to_u64(value: U256, field_name: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| anyhow!("{field_name} overflows u64: {value}"))
}

fn parse_calldata(value: &str) -> Result<Vec<u8>> {
    let normalized = value.strip_prefix("0x").unwrap_or(value);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(normalized).map_err(|err| anyhow!("invalid calldata hex {value}: {err}"))
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Bytes, U256};

    use super::{
        DecodedAction, SolCall, decode_action, decode_root_cid, publishDappCall, upgradeDappCall,
    };

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
        let call = publishDappCall {
            rootCid: Bytes::from(b"bafy123".to_vec()),
            name: "App".to_string(),
            version: "1.0.0".to_string(),
            description: "desc".to_string(),
        };

        let decoded = decode_action(
            &["0xfb84b57e757649dff3870f1381c67c9097d0c67f".to_string()],
            &[format!("0x{}", hex::encode(call.abi_encode()))],
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
        let call = upgradeDappCall {
            dappId: U256::from(42u64),
            rootCid: Bytes::from(b"bafy-upgrade".to_vec()),
            name: "App".to_string(),
            version: "2.0.0".to_string(),
            description: "desc2".to_string(),
        };

        let decoded = decode_action(
            &["0xfb84b57e757649dff3870f1381c67c9097d0c67f".to_string()],
            &[format!("0x{}", hex::encode(call.abi_encode()))],
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
