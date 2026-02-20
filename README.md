# governance-agent

Rust long-running process for VibeFi DAO governance proposal review and optional vote execution.

## Current implementation status

This repository includes a working foundation through vote execution:

- CLI commands: `run`, `review-once`, `backfill`, `doctor`, `config print`
- Layered config defaults for `devnet` and `sepolia`
- `alloy`-based chain adapter and signer execution (no `ethers`/`ethabi`)
- HTTP/WS autodetect based on `rpc_url` scheme (`http(s)` vs `ws(s)`)
- Decoding of `DappRegistry.publishDapp` and `upgradeDapp` calldatas
- Root CID extraction (UTF-8 first, hex fallback)
- IPFS `manifest.json` fetch + shared CID cache (compatible with client cache layout)
- Lightweight source/script checks and LLM context enrichment
- Decision engine with numeric thresholds and optional profile aliases
- Keystore-backed vote submission (`castVoteWithReason`) with preflight checks, plus dry-run mode
- LLM callouts for OpenAI, Anthropic, and OpenCode-compatible APIs with automatic provider fallback
- LLM audit persistence with prompt/response redaction
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
  - `GOV_AGENT_MIN_VOTE_BLOCKS_REMAINING`
  - `GOV_AGENT_MAX_GAS_PRICE_GWEI`
  - `GOV_AGENT_MAX_PRIORITY_FEE_GWEI`
  - `GOV_AGENT_APPROVE_THRESHOLD`
  - `GOV_AGENT_REJECT_THRESHOLD`
  - `GOV_AGENT_DECISION_PROFILE`
  - `GOV_AGENT_IPFS_CACHE_DIR`
  - `GOV_AGENT_DATA_DIR`

## Notes

- Default mode is dry-run recommendation.
- Auto-vote requires keystore configuration and sends `castVoteWithReason` after preflight:
  - `state == Active`
  - `hasVoted == false`
  - enough blocks remain before `voteEnd`
  - gas/priority fee are under configured caps
- Default IPFS cache path is `~/.cache/VibeFi`, so governance-agent can reuse bundle artifacts cached by the client on the same machine.
