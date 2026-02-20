# governance-agent

Rust long-running process for VibeFi DAO governance proposal review and optional vote execution.

## Current implementation status

This repository now includes a working Phase 1/2 foundation:

- CLI commands: `run`, `review-once`, `backfill`, `doctor`, `config print`
- Layered config defaults for `devnet` and `sepolia`
- JSON-RPC polling of `VfiGovernor` `ProposalCreated` logs
- Decoding of `DappRegistry.publishDapp` and `upgradeDapp` calldatas
- Root CID extraction (UTF-8 first, hex fallback)
- IPFS `manifest.json` fetch, lightweight source/script checks, and LLM context enrichment
- Decision engine with `conservative` / `balanced` / `aggressive` profiles
- Keystore-backed vote submission (`castVoteWithReason`) with preflight checks, plus dry-run mode
- LLM callouts for OpenAI, Anthropic, and OpenCode-compatible APIs with automatic provider fallback
- JSON-file state persistence and block cursoring

## Usage

```bash
cargo run -- config print
cargo run -- doctor --profile devnet --rpc-url http://127.0.0.1:8545
cargo run -- run --profile devnet --rpc-url http://127.0.0.1:8545 --once
cargo run -- review-once --proposal-id 1 --profile sepolia --rpc-url "$SEPOLIA_RPC_URL"
cargo run -- backfill --from-block 10239268 --profile sepolia --rpc-url "$SEPOLIA_RPC_URL"
```

## Configuration

- Optional file: `--config config/example.toml`
- Env overrides:
  - `GOV_AGENT_PROFILE`
  - `GOV_AGENT_RPC_URL`
  - `GOV_AGENT_GOVERNOR`
  - `GOV_AGENT_DAPP_REGISTRY`
  - `GOV_AGENT_AUTO_VOTE`
  - `GOV_AGENT_KEYSTORE_PATH`
  - `GOV_AGENT_KEYSTORE_PASSWORD`
  - `GOV_AGENT_DATA_DIR`

## Notes

- Default mode is dry-run recommendation.
- Auto-vote requires keystore configuration and sends `castVoteWithReason` after preflight (`state == Active`, `hasVoted == false`).
