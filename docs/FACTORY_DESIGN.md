# Thee Pori Software Factory: Design v2

Status: approved 2026-10-19. Source: Software Factory PRD v2 (Tauri desktop + mobile). This file records **how** we build it; the PRD says **what**.
Supersedes v1 (2026-10-17, see git history) where they conflict. Decisions are recorded in [`adr/0001-prd-v2-architecture.md`](adr/0001-prd-v2-architecture.md).
Work is tracked in Jira project **SF** (epics SF-1..SF-12 = M0..M11).

## 1. Scope and constraints

- **Personal tool.** One founder, one Mac. TP drives the locally installed `claude` CLI under the founder's own claude.ai login. Not sold, no third-party users, no API-key mode.
- **Target build: this Mac only**: MacBookPro15,1, Intel Core i7-9750H, 32 GB, macOS 15.7.9 (x86_64). CI runs tests on `macos-latest`. No Linux, Windows or iOS builds or code paths.
- **Mobile: Flutter, Android only**, reaching the Mac through a relay on the founder's Cloudflare account.
- Everything in the PRD is in scope; work is sequenced by dependency (§12), not cut.

## 2. Shape of the system

```
Tauri app (React/TS) ─┐                          ┌─ Flutter Android app
tp CLI ───────────────┼─ Unix socket JSON-RPC    │   (E2E, device keys)
                      ▼                          ▼
                    tpd  ◀── outbound WSS ── CF Worker + Durable Object relay ◀── GitHub webhooks, FCM
  ├ engine      SQLite (WAL): projects, tasks, runs, state machine, outbox, approvals, usage, FTS5
  ├ scheduler   intake → dedupe/correlate → daily plan → assignment (concurrency + worktree locks)
  ├ manager     stateless LLM proposals (triage, plan, explanations, chat) → policy gate
  ├ policy      rules + deterministic risk classifier; hook endpoint (fail-closed)
  ├ runtime     trait Runtime; claude = CLI adapter, generated settings, PTY attach for intervention
  ├ github      gh CLI adapter (deterministic)       ├ jira   steward runs via Atlassian Rovo MCP
  ├ verify      verify.yaml runners + reviewer agent  ├ deploy adapters, smoke, rollback
  ├ vault       ported Panuval crypto, key agent, leases, secrets broker
  ├ mcp         tp MCP server for employees
  ├ audit       hash-chained JSONL + SQLite index
  ├ memory      scoped lessons with provenance        ├ portability  export/import/doctor/resume
  └ analytics   source-tagged usage, versioned pricing, budgets
```

| Component | Language | Notes |
|---|---|---|
| `crates/tp-core` | Rust lib | All factory logic. Used by every binary. |
| `crates/tpd` | Rust bin | The daemon. launchd user agent (`KeepAlive`). Owns all work; closing the GUI never stops anything. |
| `crates/tp-cli` | Rust bin `tp` | Setup, doctor, project registry, daemon control, hook entrypoint. |
| `crates/tp-proto` | Rust lib | IPC/remote message types (serde). TS types generated for the UI; Dart types for mobile. |
| `apps/desktop` | Tauri 2 + Vite + React + TS | A client of tpd. Rust side does async socket IO; the UI thread never blocks. |
| `apps/mobile` | Flutter (Android) | Client of tpd through the relay. |
| `relay/` | Cloudflare Worker + Durable Object | Sees only ciphertext, heartbeats and push tokens. |

### Factory layout (`~/factory`, override with `TP_FACTORY`)

```
factory.yaml            global config + global policy (schema-versioned)
state.db                SQLite: engine, audit index, usage, memory
tpd.sock                IPC socket (0600)       tpd.lock  single-instance lock
logs/                   tpd logs
audit/YYYY-MM.jsonl     append-only, hash-chained
vault/                  encrypted vault (sandbox-denied to agents)
skills/  memory/global/
projects/<slug>/        project.yaml, verify.yaml, skills/, worktrees/<task>/, evidence/<run>/
```

## 3. Claude Code runtime (employees)

- **Interface:** the `claude` CLI subprocess. There is no official Rust SDK, and the Agent SDK targets TS/Python and API keys. Coding runs: `claude -p --input-format stream-json --output-format stream-json --verbose --setting-sources user --settings <generated> --strict-mcp-config --mcp-config <tp mcp>` in the task worktree.
- **Capabilities** are read from `system/init` (`capabilities`, tools, MCP status) and recorded per run. Gaps are explicit, never papered over: see `docs/runtime/claude-code-capabilities.md` (M1).
- **Intervention:** interrupt = SIGINT (ends the turn, session kept); continue = `claude -p --resume <session>`; hands-on = PTY in the Coding Workspace running `claude --resume <session>`. No private control protocol.
- **Background sessions** started by the founder (`claude --bg`) are listed read-only through `claude agents --json`.
- **Remote Control** is optional ("open in Claude"), not a dependency.

## 4. Enforcement (outside the LLM)

Per run, tp generates a settings file outside the worktree. Rules from the spikes still hold:
- Always `--setting-sources user`: a repo `.claude/settings.json` with `disableAllHooks` otherwise silently disables tp's hooks (spike 2).
- PreToolUse hook → tpd policy. HTTP hook with `onFailure: block` once Claude Code ≥ 2.1.295; until then a command hook `tp hook pre` that talks to tpd over the socket and **fails closed** (unreachable daemon = deny).
- macOS sandbox for Bash plus `permissions.deny` for file tools: `~/factory/vault`, `state.db`, the settings file, `~/.claude/**`, `.env*`.
- Coding runs may not run `gh` or `git push`. Only tpd does GitHub writes.
- Deny beats allow. Project config may only tighten. No agent, including the manager, can change policy.

Risk levels L1–L3 and "who authorized it" recording are unchanged from v1 §6.

## 5. Integrations

### GitHub: `gh` CLI, run by tpd
- tpd invokes `gh` itself (no LLM in the loop), reusing `gh`'s stored login. tp never reads or stores the token.
- Reads: `gh api` with ETag / `If-None-Match` for reconciliation (304s are free), at a bounded interval.
- Events: repo webhooks created with `gh api`, delivered to the relay, verified with an HMAC secret held in the vault, forwarded to tpd. Missed events are caught by reconciliation.
- Writes: branch push, `gh pr create|edit|merge`, labels, comments, all through the outbox with idempotency keys and the merge gate (§7).
- Dedup: every issue/PR carries label `tp-<task-id>`; delivery IDs are stored.

### Jira: Atlassian Rovo MCP through Claude's connector
- tp does not hold Atlassian credentials. Jira operations run as **steward** runs: `claude -p` with a cheap model, `--allowedTools 'mcp__claude_ai_Atlassian__*'`, the policy hook, `--json-schema` output, the cached cloudId in the prompt.
- Every write is followed by a read-only confirm call. Only a confirmed result marks the outbox intent `synced`; otherwise bounded retry, then `blocked`.
- Pending intents for a project are batched into one steward run. Issue text read by a steward is data, never instructions.
- `pending` connector status at init is normal (tools load through ToolSearch).

## 6. Manager and task flow

- **Manager = deterministic engine + stateless LLM calls.** Each call (`claude -p --json-schema`) receives a state snapshot from SQLite and returns proposals (triage, correlation, plan items, plain-language approval text, chat replies). Proposals pass the policy gate before anything happens. Chat threads persist in SQLite. Restarting tpd loses nothing.
- **Daily plan:** signals collect all day; at the configured time one plan is drafted; one approval authorizes every item within its risk class. Critical security issues and production incidents escalate immediately.
- **State machine:** `received → triaged → planned → approved → jira_linked → criteria_ready → implementing → verifying ⇄ repairing → pr_open → awaiting_approval? → merged → deploying → post_verify → done`; side exits `blocked · escalated · rolled_back · cancelled`. Each transition is one DB transaction + one audit event; side effects go through the outbox.
- One run per task, one worktree per task, file-level conflict check before assignment, global and per-project concurrency limits, budgets as hard stops.

## 7. Verification and merge gate

Unchanged from v1 §7 (verify.yaml, statuses, missing evidence = not passed, Android device selection, Playwright). Additions:
- Layer 4: a separate reviewer run (fresh context, read-only tools) produces findings; findings never count as proof.
- **Merge gate** (deterministic): required checks green, no blocking findings, risk ≤ repo auto-merge policy, branch protection respected (never bypassed), authorization ID recorded.
- Repair loop bounded by attempts, minutes and budget, then escalate.

## 8. Remote access

- **Relay:** Cloudflare Worker + one Durable Object per factory. tpd keeps an outbound WebSocket; the phone connects to the same DO. The DO stores ciphertext queues (bounded), the last tpd heartbeat (so the app can say "desktop offline" honestly) and FCM tokens.
- **Pairing:** done locally. The desktop shows a QR code with the relay URL, a one-time pairing token and tpd's X25519 public key. The phone generates its keys in Android Keystore. Messages are sealed end to end (X25519 + ChaCha20-Poly1305); every command is signed by the device key and checked against that device's permission scope (view / approve / control).
- **Push:** FCM payloads carry no content, only "something needs you" + an opaque id; the app fetches details over E2E.
- **Revocation:** removing a device on the desktop drops its key in tpd and the relay. Every remote action is audited with the device id.
- **Remote vault unlock:** at pairing, tpd wraps the vault DEK with a random secret U, hands U to the phone (local, during pairing) and forgets it. The phone stores U encrypted by a Keystore key that requires biometrics. To unlock remotely, the founder approves a scoped lease (project/environment/TTL) with biometrics; the phone sends U over E2E; tpd loads a scoped lease into the key agent. No unattended key.
- Emergency controls (pause assignments, stop sessions, disable auto-merge/deploy, revoke remote, safe mode) are available from desktop, CLI and phone.

## 9. Vault, audit, memory, portability, analytics

- **Vault:** port of Panuval crypto (Argon2id KEK → AES-256-GCM DEK → per-item AES-GCM with AAD), key agent inside tpd with TTL and idle wipe, scoped leases, runtime injection only. Rust must decrypt Go-produced test vectors byte-for-byte before Panuval data is migrated.
- **Audit:** versioned schema (`schema: 1`), monthly JSONL, SHA-256 hash chain, SQLite index, `verify_chain`. Every event is tagged `observed` (tp saw it) or `claimed` (an agent said it). No secrets, bodies or file contents.
- **Memory, export/import, analytics:** as v1 §11–§13, minus Remote Control and tunnels. Usage cost on a subscription is labelled "estimated at list price".

## 10. Performance

Targets from PRD §16, measured on release builds on this Mac: warm start p95 ≤ 500 ms, cold start to usable shell p95 ≤ 1 s, interaction p95 ≤ 50 ms, 60 FPS. All data reaches the UI through IPC streams; lists, logs and diffs are virtualized; no blocking work on the UI thread. `scripts/perf` records results with the machine description.

## 11. Reused from the Go agent (branch `legacy/go-agent`, tag `go-agent-final`)

| Go | Rust | Pin |
|---|---|---|
| `internal/permission` classifier | `tp_core::policy::classify` | Go table tests ported verbatim |
| `internal/secrets` | `tp_core::redact` | same cases |
| `internal/config` layering, tighten-only | `tp_core::config` | same cases |
| `internal/workspace` | `tp_core::workspace` | same cases |
| `internal/credentials` (keychain) | `keyring` crate | – |
| Panuval `crypto` / `agent` / `vault` / audit chain | `tp_core::{vault,audit}` | Go test vectors |

## 12. Build order

| M | Jira | Scope | Gate |
|---|---|---|---|
| M0 | SF-1 | Workspace, tpd + launchd, SQLite, registry, audit chain, factory.yaml, `tp setup/doctor`, Tauri shell, perf harness, CI | Setup idempotent; GUI close leaves tpd running; startup measured |
| M1 | SF-2 | Runtime trait, Claude adapter, worktrees, generated settings, hooks → policy, capability doc | Policy-bypass suite |
| M2 | SF-3 | Coding Workspace + Mission Control v1, PTY, diffs, FTS5 search; freeze `tp-proto` | p95 < 50 ms on a 100k-line log |
| M3 | SF-4 | gh adapter + reconciliation + webhooks; Jira stewards + outbox | Redelivered events → exactly one task/issue/PR |
| M4 | SF-5 | Manager, scheduler, daily plan, Approval Center, assignment, budgets | Fixture issue → approved once → runs unattended |
| M5 | SF-6 | Verification 1–4, reviewer, repair loop, merge gate, CI monitoring | No merge without evidence; unauthorized merge blocked |
| M6 | SF-7 | Vault port, broker, tp MCP, Panuval migration | Canary secret never leaks |
| M7 | SF-8 | Relay, pairing, E2E, Flutter app, FCM, remote unlock, emergency controls | Offline shown as offline; revoked device rejected |
| M8 | SF-9 | Dependency/security pipeline; deploy adapters, rollback | Failed health check → rollback, traceable |
| M9 | SF-10 | Memory, export/import/resume, vault export | Clean round trip; no duplicate runs |
| M10 | SF-11 | Analytics, pricing versions, budgets, learning loop | Cost uses record's pricing version |
| M11 | SF-12 | Hardening, AC 1–17 suite, perf validation | All 17 AC pass |

M7 can run in parallel once `tp-proto` is frozen (end of M2).

## 13. Open

1. Pilot project (needed by the M4 gate).
2. Claude Code ≥ 2.1.295 for fail-closed HTTP hooks; command-hook fallback until then.
