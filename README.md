# gov-agent

Rust long-running process for VibeFi DAO governance proposal review and optional vote execution.

## Current implementation status

This repository includes a working foundation through vote execution:

- CLI commands: `run`, `review-once`, `backfill`, `status`, `config print`
- Layered config defaults for `devnet` and `sepolia`
- `alloy`-based chain adapter and signer execution (no `ethers`/`ethabi`)
- HTTP/WS autodetect based on `rpc_url` scheme (`http(s)` vs `ws(s)`)
- Decoding of `DappRegistry.publishDapp` and `upgradeDapp` calldatas
- Root CID extraction (UTF-8 first, hex fallback)
- IPFS `manifest.json` fetch + shared CID cache (compatible with client cache layout)
- Lightweight source/script checks and LLM context enrichment with bundle file index + text content snapshot
- Graceful shutdown on Ctrl+C / SIGTERM for daemon mode
- Decision engine with numeric thresholds and optional profile aliases
- Keystore-backed vote submission (`castVoteWithReason`) with preflight checks, plus dry-run mode
- LLM callouts for OpenAI, Anthropic, Ollama, and VeniceAI with automatic provider fallback
- LLM audit persistence with prompt/response redaction
- JSON-file state persistence and block cursoring

## Usage

```bash
cargo run -- config print
cargo run -- status --profile devnet --rpc-url http://127.0.0.1:8545
cargo run -- run --profile devnet --rpc-url http://127.0.0.1:8545 --once
cargo run -- review-once --proposal-id 1 --profile sepolia --rpc-url "$SEPOLIA_RPC_URL"
cargo run -- backfill --from-block 10239268 --profile sepolia --rpc-url "$SEPOLIA_RPC_URL"
```

Quick malicious-bundle proposal flow (via `vibefi/e2e`) for fast gov-agent pickup testing:

```bash
bun run publish:test-bundle -- --bundle red_team_vapp
bun run publish:test-bundle -- --bundle malicious_uniswapv2
```

## Configuration

- Optional file: `--config config/example.toml`
- For `devnet`, defaults can be loaded from `contracts/.devnet/devnet.json` (or `GOV_AGENT_DEVNET_JSON` override), using:
  - `chainId` -> `network.chain_id`
  - `vfiGovernor` -> `network.governor_address`
  - `dappRegistry` -> `network.dapp_registry_address`
- Env overrides:
  - `GOV_AGENT_PROFILE`
  - `GOV_AGENT_RPC_URL`
  - `GOV_AGENT_DEVNET_JSON`
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
  - `GOV_AGENT_DETERMINISTIC_WEIGHT`
  - `GOV_AGENT_LLM_WEIGHT`
  - `GOV_AGENT_FROM_BLOCK`
  - `GOV_AGENT_IPFS_CACHE_DIR`
  - `GOV_AGENT_DATA_DIR`

## Notes

- Default mode is dry-run recommendation.
- `network.chain_id`, `network.governor_address`, and `network.dapp_registry_address` are required. The process exits early when missing/invalid.
- Auto-vote requires keystore configuration and sends `castVoteWithReason` after preflight:
  - `state == Active`
  - `hasVoted == false`
  - enough blocks remain before `voteEnd`
  - gas/priority fee are under configured caps
- Default IPFS cache path is `~/.cache/VibeFi`, so gov-agent can reuse bundle artifacts cached by the client on the same machine.
