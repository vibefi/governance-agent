# governance-agent build plan

## Objective
Build a Rust long-running process that watches `VfiGovernor`, extracts dapp proposal payloads, reviews proposal bundles with LLM/tool pipelines, and outputs or submits votes (`for`/`against`/`abstain`).

## Decisions locked from this thread
- Vote mode: default is `dry-run` recommendation-only; auto-vote is opt-in via CLI flag/env var.
- Confidence policy: configurable; include 3 built-in defaults (`conservative`, `balanced`, `aggressive`).
- Launch environments: local devnet (matching `e2e` flow) + Sepolia support.
- Signer priority: keystore first; provider interface must support adding KMS/HSM later.
- LLM support on day 1: OpenAI, Anthropic, OpenCode-compatible endpoint.
- Alerting on day 1: structured logs + Telegram; notifier interface remains transport-agnostic.
- Single reviewer per instance, multi review will be achieved by user running multiples instances.

## Concrete implementation details discovered in this repo
- Proposal source is `VfiGovernor` `ProposalCreated` logs with full `targets`, `values`, `calldatas`, `description`.
- For dapp governance actions, the bundle CID is in proposal calldata, not in proposal description:
  - `DappRegistry.publishDapp(bytes rootCid, string name, string version, string description)`
  - `DappRegistry.upgradeDapp(uint256 dappId, bytes rootCid, string name, string version, string description)`
- CLI currently encodes raw CID strings to bytes before proposing (`encodeRootCid`), so decoding should attempt UTF-8 first and fall back to raw hex.
- E2E local path is `bun run e2e`, which drives: package -> propose -> vote -> queue -> execute -> fetch/verify.
- Sepolia addresses and chain metadata are documented in `docs/docs/reference/contract-addresses.md`.

## High-level architecture
- `agent-core`: lifecycle, scheduler, run modes (`daemon`, `review-once`), graceful shutdown.
- `chain-adapter`: event indexing, proposal state polling, vote submission (use `alloy` library).
- `proposal-decoder`: filters target contracts and decodes dapp calldata to typed actions (use `alloy` library).
- `bundle-fetcher`: IPFS retrieval, CID/manifest checks, workspace materialization.
- `review-engine`: deterministic checks + LLM orchestration + optional tool execution (allow configurable prompts to be loaded).
- `decision-engine`: policy + confidence thresholds + final vote decision.
- `signer-provider`: keystore implementation first, provider trait for future KMS/HSM.
- `llm-provider`: OpenAI, Anthropic, OpenCode adapters behind one trait.
- `notifier`: logs + Telegram implementations behind transport trait.
- `storage`: JSON files (default to directory in home, configurable) - it won't be high-volume as it's always limited to ethereum block time

## Implementation phases

### Phase 0: ADRs + contracts/docs alignment
- Write ADRs for:
  - proposal decoding strategy (from `ProposalCreated.calldatas`)
  - vote execution mode gating (`dry-run` default, explicit submit enablement)
  - signer provider abstraction
  - LLM provider abstraction and audit logging
  - confidence model + fallback policy
- Record concrete parsing rules for `publishDapp` and `upgradeDapp`.

Deliverables:
- ADR set and parsing specification with test vectors from current ABI signatures.

### Phase 1: Scaffold + config surface
- Create Rust workspace/package skeleton (later step).
- Add CLI:
  - `run` (daemon)
  - `review-once --proposal-id`
  - `backfill`
  - `doctor`
  - `config print`
- Add layered config (`defaults -> file -> env -> cli`) with typed schema.
- Add explicit execution gate:
  - `--auto-vote` flag
  - `GOV_AGENT_AUTO_VOTE=true` env

Deliverables:
- Process starts, config loads, and run mode is visible in logs.

### Phase 2: Chain listener + durable state
- Subscribe/poll `ProposalCreated` on `VfiGovernor`.
- Persist proposals and processing state backed by JSON files:
  - proposal record, decoded action, review status, decision, vote tx metadata
  - reorg-safe cursor/checkpoint
- Add idempotent replay/backfill command.

Deliverables:
- New proposals are discovered and persisted across restarts and reorgs.

### Phase 3: Dapp proposal decoding + CID extraction
- Decode proposal actions using `DappRegistry` ABI.
- Accept only recognized action types for v1:
  - `publishDapp`
  - `upgradeDapp`
- CID extraction algorithm:
  - parse `rootCid` bytes from calldata
  - decode as UTF-8 CID when valid (current CLI path)
  - fallback to hex bytes representation
- Mark unsupported/multi-target proposals for policy fallback.

Deliverables:
- Deterministic `DecodedProposal` artifact with `action_type`, `dapp_id?`, `root_cid`, metadata.

### Phase 4: IPFS fetch + bundle validation
- Fetch CID from configured gateways with retries/time budgets.
- Validate:
  - CID and byte-size limits
  - required manifest presence
  - path traversal and unsafe file checks
  - deterministic local layout for review
- Cache fetched bundles keyed by CID.

Deliverables:
- Review-ready local bundle tree per proposal.

### Phase 5: Review pipeline (LLM + tools)
- Deterministic pre-check stage:
  - policy lint, forbidden patterns, dependency/permission scan
  - summary artifact for LLM context
- Provider-agnostic LLM stage:
  - OpenAI adapter
  - Anthropic adapter
  - OpenCode-compatible adapter
- Custom `prompt.md` injection
- Persist model name/version/prompts/responses (with secret redaction).

Deliverables:
- Structured review report independent of provider.

### Phase 6: Decision engine + confidence profiles
- Unified decision schema:
  - `recommended_vote`
  - `confidence`
  - `reasons`
  - `blocking_findings`
  - `requires_human_override`
- Configurable confidence threshold + abstain band.
- Ship 3 defaults:
  - `conservative`: require high confidence (recommended default for auto-vote off)
  - `balanced`: medium threshold for active operation
  - `aggressive`: lower threshold, abstain less frequently

Deliverables:
- Deterministic mapping from review output to vote recommendation.

### Phase 7: Vote execution + signer providers
- Implement signer provider trait with keystore backend first.
- Add vote pre-flight checks:
  - proposal state still votable
  - no prior vote cast by signer
  - time-to-deadline buffer
  - gas/fee safety limits
- If auto-vote enabled, submit `castVoteWithReason` and persist tx receipt.

Deliverables:
- Safe, idempotent vote submission path gated by explicit opt-in.

### Phase 8: Observability + notifications
- Structured JSON logs and metrics.
- Notifier trait and transports:
  - logs (default)
  - Telegram (day 1)
  - extensible for Discord/webhook later
- Alerts for:
  - proposal detected
  - decode/fetch/review failures
  - high-risk findings
  - vote submitted/failed
  - stale listener

Deliverables:
- Operational visibility for local and hosted runs.

### Phase 9: Environment profiles + packaging
- `devnet` profile:
  - tuned for local `bun run e2e` style iteration
  - local RPC/IPFS defaults
- `sepolia` profile:
  - chain id `11155111`
  - contract defaults sourced from docs addresses
- Packaging:
  - native binary
  - Docker image (non-root, minimal runtime)

Deliverables:
- One-command local test run and reproducible Sepolia config bootstrap.

### Phase 10: Test and hardening
- Unit tests:
  - calldata decoding and CID extraction
  - confidence policy decisions
  - signer provider behavior
- Integration tests:
  - local devnet + mocked LLM + IPFS
  - end-to-end proposal -> decode -> fetch -> review -> decision
- Reliability tests:
  - RPC/IPFS outages
  - LLM timeout/rate-limit behavior
  - restart mid-pipeline

Deliverables:
- CI gate with deterministic coverage of the governance-agent critical path.

## Initial default config
- Storage: JSON
- Network profile: `devnet`
- Vote mode: `dry-run`
- Auto-vote gate: disabled unless `--auto-vote` or `GOV_AGENT_AUTO_VOTE=true`
- LLM providers enabled: OpenAI, Anthropic, OpenCode adapters
- Alerts: logs + Telegram

## Sepolia bootstrap constants (from docs)
- Chain ID: `11155111`
- VfiGovernor: `0x753d33e2E61F249c87e6D33c4e04b39731776297`
- DappRegistry: `0xFb84B57E757649Dff3870F1381C67c9097D0c67f`
- VfiToken: `0xD11496882E083Ce67653eC655d14487030E548aC`
- VfiTimelock: `0xA1349b43D3f233287762897047980bAb3846E23b`
- ProposalRequirements: `0x641d8C8823e72af936b78026d3bb514Be3f22383`

## Milestones and acceptance criteria
1. `M1 Decode`: agent reliably extracts `rootCid` from proposal calldata.
2. `M2 Review`: deterministic review report generated for each supported proposal.
3. `M3 Decision`: confidence policy consistently maps to vote recommendation.
4. `M4 Execute`: optional auto-vote submits safely and idempotently.
5. `M5 Operate`: devnet and Sepolia profiles run with logs + Telegram alerts.
