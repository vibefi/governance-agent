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
- Prometheus metrics endpoint and OpenTelemetry trace export hooks

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
bun run publish:test-bundle red_team_vapp
bun run publish:test-bundle malicious_uniswapv2
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
  - `GOV_AGENT_MINIFY_BUNDLE_TEXT`
  - `GOV_AGENT_IPFS_CACHE_DIR`
  - `GOV_AGENT_DATA_DIR`
  - `GOV_AGENT_METRICS_ENABLED`
  - `GOV_AGENT_METRICS_BIND`
  - `GOV_AGENT_OTLP_ENDPOINT`
  - `GOV_AGENT_OTLP_SERVICE_NAME`
  - `GOV_AGENT_OTLP_TIMEOUT_SECS`

## Observability

- Prometheus exporter:
  - Enabled by default on `127.0.0.1:9464/metrics`
  - Configure via `observability.metrics_enabled` and `observability.metrics_bind`
- OpenTelemetry traces:
  - Enable by setting `observability.otlp_endpoint` (or `GOV_AGENT_OTLP_ENDPOINT`)
  - Proposal lifecycle spans include `proposal_id` and stage-level timings

Dashboard-ready metrics:

- `gov_agent_proposals_discovered_total`
- `gov_agent_proposals_processed_total`
- `gov_agent_proposals_failed_total{stage=...}`
- `gov_agent_stage_latency_seconds{stage=decode|fetch_proposals|review|vote_submit|...}`
- `gov_agent_vote_submit_total{status=success|failure}`
- `gov_agent_provider_errors_total{provider=rpc|ipfs|llm|decoder,operation=...}`
- `gov_agent_last_successful_poll_timestamp_seconds`
- `gov_agent_last_poll_attempt_timestamp_seconds`
- `gov_agent_last_processed_proposal_timestamp_seconds`
- `gov_agent_listener_staleness_seconds`

Critical alert examples:

- Listener stale: `time() - gov_agent_last_successful_poll_timestamp_seconds > 120`
- Repeated stage failures: rate on `gov_agent_proposals_failed_total{stage=...}`
- Vote submit failures: rate on `gov_agent_vote_submit_total{status="failure"}`

### Local test stack (Prometheus + Grafana + Tempo + OTel Collector)

This repo includes a ready-to-run local stack under [`observability/`](./observability):
- Prometheus on `http://127.0.0.1:9090`
- Grafana on `http://127.0.0.1:3000` (default login `admin` / `admin`)
- Tempo on `http://127.0.0.1:3200`
- OTel Collector OTLP ingest on `127.0.0.1:4317` (gRPC), `127.0.0.1:4318` (HTTP)

Start the stack:

```bash
cd observability
docker compose up -d
```

Run `gov-agent` with telemetry enabled:

```bash
GOV_AGENT_METRICS_ENABLED=true \
GOV_AGENT_METRICS_BIND=127.0.0.1:9464 \
GOV_AGENT_OTLP_ENDPOINT=http://127.0.0.1:4317 \
GOV_AGENT_OTLP_SERVICE_NAME=gov-agent-local \
cargo run -- run --profile devnet --rpc-url http://127.0.0.1:8545
```

Quick checks:

```bash
curl -s http://127.0.0.1:9464/metrics | rg '^gov_agent_'
curl -s 'http://127.0.0.1:9090/api/v1/query?query=rate(gov_agent_proposals_processed_total%5B5m%5D)'
```

Grafana provisions:
- `Prometheus` datasource
- `Tempo` datasource
- `Gov Agent Observability` dashboard
- Starter Prometheus alert rules:
  - `GovAgentListenerStale`
  - `GovAgentVoteSubmitFailures`
  - `GovAgentRepeatedStageFailures`

Check active alerts:

```bash
curl -s http://127.0.0.1:9090/api/v1/rules
curl -s http://127.0.0.1:9090/api/v1/alerts
```

Stop/remove stack:

```bash
cd observability
docker compose down -v
```

## Local Models

Ships with support for a local [Ollama](https://ollama.com/) API server.

- `qwen3.5:9b` with 32k context window size works very well in preliminary testing.

## Notes

- Default mode is dry-run recommendation.
- `network.chain_id`, `network.governor_address`, and `network.dapp_registry_address` are required. The process exits early when missing/invalid.
- Auto-vote requires keystore configuration and sends `castVoteWithReason` after preflight:
  - `state == Active`
  - `hasVoted == false`
  - enough blocks remain before `voteEnd`
  - gas/priority fee are under configured caps
- Default IPFS cache path is `~/.cache/VibeFi`, so gov-agent can reuse bundle artifacts cached by the client on the same machine.
