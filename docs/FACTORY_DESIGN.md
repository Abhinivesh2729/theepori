# Thee Pori Software Factory: Design

Status: finalized in brainstorm, 2026-10-17. Not implemented yet.
Source: Software Factory PRD v1.0. This file records **how** we build it and why; the PRD says **what**.

## 1. Decisions

| # | Decision | Consequence |
|---|---|---|
| D1 | tp is **only** a software factory. The built-in coding agent is removed. | Delete `agent`, `llm/*`, `providers`, `tools`, `tui`, REPL, sessions. Ship as v1.0.0 (breaking). |
| D2 | **Thinai is dropped** from tp. | Remove Thinai and Ollama code, docs and site claims. |
| D3 | **Claude Code is the only coding agent**, through an adapter interface. | tp spawns `claude`, never calls a model API itself. |
| D4 | **Jira only through Claude Code's Atlassian connector.** tp never talks to Jira. | Steward pattern (§4). |
| D5 | **GitHub only through Claude Code**, using the **`gh` CLI** run by Claude (stewards and cockpit). | Same steward pattern. Local `git` (worktrees, diff, status) stays in tp: it is local, not GitHub. |
| D6 | **tp vault replaces Panuval.** Port most of its code, rename `panuval` → `tp vault`. | Firebase sync and Google login removed. Local-only. |
| D7 | **Retire Panuval CLI and Panuval Android.** | Migrate existing secrets into tp vault first, then archive both repos and pull the Android build. |
| D8 | **Remote access through Cloudflare Tunnel, on demand only.** Tunnel goes down when the work is done. | Needs a fixed hostname (passkeys are bound to a domain), see §9. |
| D9 | **Passkey (WebAuthn)** authenticates the dashboard and the vault web UI. | |
| D10 | **Agent isolation = Claude Code sandbox + tp hooks.** No separate OS user. | §6. |
| D11 | **Android verification device:** physical device over adb when one is connected, or when the feature needs one; otherwise the emulator. | §7.3. |
| D12 | **Everything in the PRD is in scope.** No deferrals. Work is sequenced by dependency (§14), not cut. | |
| D13 | Pilot project: chosen later. | |

## 2. Shape of the system

```
tp (single Go binary)
├─ tp daemon       launchd service: scheduler, intake polling → queue, run supervisor,
│                  outbox worker, dashboard + vault web UI, tunnel + Remote Control supervisor
├─ engine          SQLite ~/factory/state.db: projects, tasks, runs, state machine,
│                  idempotency keys, outbox, approvals, usage
├─ adapter/claude  spawns `claude -p --output-format stream-json` per task in a git worktree;
│                  captures session id, usage and cost; resumes for repair
├─ steward         narrow Claude runs that perform Jira / GitHub operations (§4)
├─ hooks           `tp hook pre|post` wired as Claude Code PreToolUse / PostToolUse:
│                  policy enforcement + audit, outside the LLM
├─ tp mcp          tp as an MCP server for Claude Code (§5)
├─ policy          ex-`permission` classifier + risk rules + approvals
├─ verify          verify.yaml runner: build, tests, Playwright, adb/Maestro, API, health checks
├─ deploy          adapters: Cloudflare (wrangler), Firebase, Play Console, ...; rollback
├─ vault           ex-Panuval: crypto, key-holding agent, items, hash-chained audit
├─ audit           versioned events, JSONL + SQLite index, hash chain
├─ memory          lessons with provenance; export/import
├─ usage           measured usage from stream-json; versioned pricing; hours saved
├─ portability     factory export/import/doctor/resume
└─ skills          version-controlled skills installed into each project
                   (Jira/Scrum lifecycle, verification, acceptance criteria, blockers)
```

### Kept from the current repo
`cli` skeleton (cobra, minus REPL), `config` (layered, validated, comment-preserving edits), `permission` (becomes `policy`), `secrets` redaction, `credentials` (keychain), `workspace` detection, `doctor`, Makefile, GoReleaser, installer, CI secret scan.

### Removed
`agent`, `llm` (all providers, SSE, text tools, catalog), `providers`, `tools`, `tui`, `cli/repl.go`, `cli/providers_ui.go`, `session`, `ui/logo` interactive start, `tp providers`, `tp models`, `tp plan`, `tp review`, `tp tools`.

### Ported from Panuval
`crypto` (Argon2id KEK → AES-256-GCM DEK → per-item AES-GCM with AAD binding, test vectors), `agent` (key held in a background process behind a Unix socket with peer-process checks, TTL and idle wipe), `vault` (items, CRUD, hash-chained audit, `VerifyChain`), `cli/run.go` (secret injection), skill.
Dropped: `auth` (Google/Firebase), `store/remote.go`, `store/sync.go`, Firebase test fakes.

## 2.1 Session model (decided 2026-10-17)

**All factory work starts from a Claude Code Remote Control session.** That session is the cockpit: the human talks to it from phone or web, and Claude there drives tp.

- Claude in the cockpit runs `tp` commands / tp MCP tools (`tp run <task>`, `queue_next`, `task_status`).
- tp launches the coding work as child `claude -p` runs in worktrees. Children never talk to the human; anything human-facing (unlock links, approvals, blockers, results) comes back through tp to the cockpit, and Claude posts it there.
- **Intake does not start work by itself.** The daemon polls and **queues** tasks. The cockpit works the queue on request or through a **fixed-interval scheduled task** (self-paced `/loop` is not restored after restarts; recurring tasks expire after 7 days, so tp re-arms them). See docs/spikes/README.md §6.
- The cockpit's hooks and deny rules cannot come from `--settings` (Remote Control does not forward it). They must come from a settings file: user-level or managed. Open decision.
- The daemon keeps the cockpit alive: `claude remote-control --continue` under launchd, never a second session for the same factory.
- **No cockpit, no progress on human-gated steps.** If the session is gone, tasks wait in `awaiting_unlock` / `awaiting_approval`; their links are re-issued (old ones revoked) when a cockpit next asks `task_status`.

## 3. Factory layout (`~/factory`)

```
~/factory/
  factory.yaml            global config + global policy (versioned schema)
  state.db                SQLite: engine, audit index, usage, memory
  audit/YYYY-MM.jsonl     append-only, hash-chained
  vault/                  encrypted vault (sandbox-denied to agents)
  skills/                 global skills (versioned)
  memory/global/          exported/human-readable lessons
  projects/<slug>/
    project.yaml          repo, environments, deploy targets, Jira key, policy overrides
    verify.yaml           verification profile
    skills/               project skills
    worktrees/<task-id>/  isolated git worktree per task
    evidence/<run-id>/    screenshots, logs, test reports
```

`TP_FACTORY` overrides the root. Old data under `~/Library/Application Support/theepori` is not migrated (agent sessions no longer apply).

## 4. Steward pattern (Jira and GitHub through Claude)

tp owns the truth; Jira and GitHub are projections that tp keeps in sync through Claude.

1. tp writes an **intent** to the outbox: `{op: transition, target: PAN-12, to: "In Review", idempotency_key}`.
2. The outbox worker runs a **steward**: `claude -p` with a cheap model, `--strict-mcp-config`, only the needed tools allowed (`mcp__atlassian__*` for Jira; `Bash(gh …)` with a subcommand allow-list for GitHub), one exact instruction, JSON output against a schema.
3. A **second, read-only steward call** confirms the result (fetch issue, check status/PR state).
4. Only a confirmed result marks the intent `synced`. Otherwise: bounded retry with backoff, then the task goes `blocked` with the reason.

Rules:
- **No duplicates:** every Jira issue and PR carries the label `tp-<task-id>`. Every create is preceded by a JQL/GitHub search for that label.
- **Batching:** pending intents for a project go into one steward run where possible, to cut tokens.
- **Untrusted input:** issue text read by a steward is data. Stewards have no filesystem write or shell beyond what the op needs.
- **Usage** of steward runs is attributed to the task.
- **`gh` policy (hook-enforced):** coding runs may not run `gh` or `git push`; only stewards and the cockpit can. Allowed: `gh pr create|view|edit|list|checks|merge` (merge only after the policy gate), `gh issue list|view|comment|edit`, `gh label`, `git push` of the task branch. Denied everywhere: `gh auth token|login|logout`, `gh secret`, `gh repo delete`, unrestricted `gh api` (only allow-listed read endpoints). `gh` uses its own stored login; tp never sees or exports that token.
- **Intake:** the daemon polls Jira (JQL for new or updated issues in registered projects) and GitHub Issues through a steward on a configurable interval (default 15 min, only while the daemon runs). New work is queued, never started directly (§2.1). Email intake: a connector feeding the same normalize → Jira flow.

## 5. tp as an MCP server

Claude Code reaches anything privileged only through tp's own MCP tools. tp decides, executes and redacts.

| Tool | Purpose |
|---|---|
| `verify_run` | Run the project's verification profile; returns structured results and evidence paths |
| `run_with_secrets` | Run an allowed command (dev server, migration dry-run) with vault secrets injected by tp; output redacted |
| `memory_search` | Retrieve validated lessons; tp records that memory was used |
| `request_approval` | Ask the human; task pauses in `awaiting_approval` |
| `report_blocked` | Agent declares ambiguity or failure; no invented requirements |
| `task_context` | Acceptance criteria, risk level, linked Jira key |

The agent never gets a secret value back from any tool.

## 6. Policy and isolation (enforced outside the LLM)

Per run, tp generates a Claude Code settings file **outside the worktree** and passes it with `--settings`, always together with **`--setting-sources user`**. Spike 2 showed that a repo's `.claude/settings.json` with `disableAllHooks: true` silently disables tp's hooks otherwise. Coding runs also use `--strict-mcp-config` with only tp's MCP server (12k instead of 38k baseline tokens). Stewards keep connectors, restrict with `--allowedTools`, and return results through `--json-schema`.

- **PreToolUse hook → `tp hook pre`:** classifies Bash commands (existing classifier), file paths and MCP tool calls against global + project + environment policy. Deny wins. Always denied for agents: `tp vault *`, `security *` (keychain), reads of `~/factory/vault`, `~/factory/state.db`, the settings file itself, `.env*`.
- **PostToolUse hook → `tp hook post`:** writes the audit event.
- **Sandbox** (Bash only; file tools need the `permissions.deny` rules too). `gh` must be in `sandbox.excludedCommands` (Go TLS fails inside the macOS sandbox), so for stewards the hook allow-list is the only control on `gh`. Sandbox scope: filesystem limited to the worktree (+ toolchain caches); `~/factory` denied except the task's evidence directory; network allow-list per project.
- Project config and repo files may only **tighten** policy. Policy changes are audited events with the actor.

### Risk classification (deterministic)
Inputs: paths touched (auth, migrations, infra, CI, dependency manifests, secrets-adjacent files), diff size, new permissions, deploy target. The agent's own estimate can only **raise** the level.

| Level | Allowed automatically |
|---|---|
| L1 | Read, classify, create Jira items, implement in worktree, run verification, draft PR |
| L2 | Merge after checks, deploy staging, deploy low-risk production when policy conditions pass, configured recovery |
| L3 | Human approval: high-risk prod, destructive DB ops, auth/security boundaries, permission escalation, secret export, irreversible actions |

Every consequential action records **who or what authorized it** (policy rule ID or human approval ID).

## 7. Verification

tp runs verification. Claude never self-certifies.

### 7.1 Lifecycle
Define (criteria + verify profile) → Implement → Verify → Evaluate → Repair (failures + evidence sent back via `claude -p --resume`) → Repeat (bounded by `max_attempts`, `max_minutes`) → Escalate → Record → Gate.

Statuses: `passed | failed | blocked | inconclusive | not_applicable`. Missing evidence = not passed.

### 7.2 `verify.yaml`
Declares checks per type with evidence requirements: `build`, `unit`, `integration`, `contract`, `web_journeys` (Playwright specs the agent may write or extend, run by tp), `android_flows` (Maestro), `api`, `health`, `post_deploy`. Each acceptance criterion maps to at least one check, or the task cannot reach `criteria_ready`.

### 7.3 Device selection (Android)
1. Task or check marked `requires_physical_device` (camera, BLE, sensors, Play Billing, on-device inference performance, ...) → physical device over adb required; none connected → `blocked`.
2. Otherwise, physical device connected over adb → use it.
3. Otherwise → boot the emulator on demand, shut it down after the run.

Evidence: screenshots, logcat crash filter, Maestro report.

### 7.4 Web
Playwright headless by tp: user journeys, console errors, failed network requests, screenshots. Post-deploy smoke run against the deployed URL is a separate verification run.

## 8. Task state machine

```
received → triaged → jira_linked → criteria_ready → implementing
  → verifying ⇄ repairing (bounded)
  → pr_open → awaiting_approval? → merged → deploying → post_verify → done
side exits: blocked · escalated · rolled_back · cancelled
```

- Every transition is a DB transaction plus an audit event; side effects go through the outbox (idempotent).
- One active run per task; worktree per task; global and per-project concurrency limits.
- On restart (`tp factory resume`): runs with a dead process are detected by PID + heartbeat; resumable ones continue with `claude --resume <session>`, the rest go `blocked`.

## 9. tp vault

- Local-only, encrypted with the ported Panuval key hierarchy. Scopes: `project/environment`. Metadata: version, expiry, rotation status.
- **Runtime injection only** via the broker (`run_with_secrets`, verification and deploy runners). `.env` materialization only when a tool demands it: 0600, outside git, deleted after the run, never readable by the agent.
- Access decisions logged without values. Expiry warnings in dashboard and `tp factory doctor`.
- **Remote UI:** served by the daemon; `tp vault remote` (or the dashboard) brings up a **named Cloudflare Tunnel on a fixed hostname**: `vault.0xabhi.space`, on the same Cloudflare account as `tp.0xabhi.space` (configurable as `remote.hostname` in `factory.yaml`; changing it invalidates registered passkeys), passkey login, short session. The tunnel stops when the session ends, on idle timeout, or at a hard TTL, whichever comes first. It is its own named tunnel (not an ingress rule on another tunnel), and tp always runs `cloudflared` with its own `--config` (spike 5). Docs state that traffic passes through Cloudflare.
- The web UI is not a shell: it only exposes vault CRUD, approvals and read-only status.

### 9.1 Remote unlock (decided 2026-10-17)

No unattended key. When a run needs secrets and the vault is locked, the human unlocks it from any browser through a link that Claude shares in the Claude Code session.

1. Run needs secrets, vault locked → task goes `awaiting_unlock`.
2. Claude calls the tp MCP tool `request_unlock` (never `cloudflared` or a server directly; policy denies that). tp decides what is exposed.
3. tp daemon starts a minimal local web server (unlock page only) and the named Cloudflare Tunnel on the fixed hostname.
4. tool returns a one-time link: single use, ~10 min TTL, bound to this request, carrying the daemon's ephemeral X25519 public key in the URL **fragment** (fragments never reach the server or Cloudflare).
5. The request reaches the Remote Control cockpit session (§2.1), where Claude posts the link (visible on phone or web).
6. Human opens it, sees **what is requested**: task, project/environment, secret names, lease duration. Approves with a **passkey**.
7. Browser derives the unlock secret with the **WebAuthn PRF extension**, encrypts it to the daemon's ephemeral key, posts ciphertext. Cloudflare terminates TLS but only ever sees ciphertext. The master password never crosses the tunnel; it stays for local unlock and recovery.
8. Daemon unwraps the DEK into the key-holding agent as a **scoped lease** (only the approved project/environment, TTL), tunnel and server go down immediately, run resumes.
9. Timeout or denial → task stays `blocked` with reason. Every step is an audit event.

Setup: during `tp setup` (local), register a passkey with PRF and store a second DEK wrap keyed by the PRF output. More passkeys (phone, laptop) can be added; each is revocable.

The link alone is useless: a passkey is required. Still, links are single-use because the session transcript is stored by the Claude service.

The same page and flow serve **approvals** (L3 actions) — one mechanism for "human needed while away".

## 10. Audit
Versioned schema (`schema: 1`): event id, time, project, run id, task id, correlation id, actor (human / policy / agent / steward), tool, category, action, resource, result, error, authorization, related commit / verification / deployment. JSONL per month, **hash-chained** (ported from Panuval), indexed in SQLite. Default retention 30 days, configurable. No secrets, no request bodies, no file contents.

## 11. Institutional memory
SQLite table of lessons: scope (global / project), category, text, provenance (task, audit events, author), confidence, status (`proposed → validated → retired`), version. Agents propose; only a human validates (dashboard or CLI). Retrieval through `memory_search`; usage recorded. `tp memory export | import` with dedup, conflict detection, merge, and a secret scan on import.

## 12. Usage, cost and productivity
- Usage taken from Claude Code stream-json result events (input, output, cache read/write, cost) for coding runs **and** steward runs; source and confidence recorded; missing values stay missing.
- Pricing table in `factory.yaml` with effective dates and versions. Each usage record stores the pricing version used. Refresh is proposed by an agent, applied only after validation against the provider's pricing page.
- Hours saved = configurable baseline per task type − measured human time (approvals, clarifications, manual fixes). Shown with method and confidence.

## 13. Portability and Remote Control
- `tp factory export [--include-vault] | import <archive> | doctor | resume`: tar.zst with a manifest, schema versions, checksums. The vault is included only when explicitly asked for and stays encrypted. Machine-specific items (keychain entries, Claude login, connector auth, tunnel credentials, browser sessions) are never exported; import lists them for reconnection.
- Import detects paths, restores in order, dedups scheduled jobs and runs, then runs `doctor`.
- **Remote Control:** the cockpit of the factory (§2.1). The daemon runs `claude remote-control --continue` under launchd, checks for an existing session first, records it against the project in the audit log, and reports when Claude Code's own auth or consent blocks it.

## 14. Build order (all in scope; each milestone has a gate)

| M | Milestone | Gate (automated) |
|---|---|---|
| S | **Spikes** (before M1): headless `claude -p` under launchd sees the Atlassian and GitHub connectors; PreToolUse can deny MCP calls; sandbox blocks `~/factory/vault` reads; stream-json usage fields; WebAuthn PRF works on iOS Safari + Android Chrome through the tunnel; a Remote Control session can run `/loop` over the tp queue and survive daemon restarts with `--continue` | Spike notes + scripts in `docs/spikes/` |
| M0 | Teardown + foundation: remove agent code, `~/factory` layout, SQLite, project registry, versioned hash-chained audit, `tp setup` skeleton, `doctor` | Fresh setup idempotent; old tests for kept packages pass |
| M1 | Claude adapter, worktrees, hooks, policy engine, usage capture | Policy-bypass test suite; denied tool never executes |
| M2 | Steward + outbox: Jira and GitHub ops with confirm-read and idempotency | Duplicate intake creates one issue; failed confirm → retry → blocked |
| M3 | Verification engine: build/tests, Playwright, adb/emulator/Maestro, API, health; repair loop | Task cannot reach `verified` without evidence; bounded retries |
| M4 | tp vault port + broker + `tp mcp`; Panuval data migration | Secret never appears in transcript/audit in test runs; unauthorized access denied |
| M5 | Maintenance workflow end to end: intake, triage, criteria, risk, approvals, PR | Issue → verified draft PR with Jira updated, on a fixture project |
| M6 | Deploy adapters, post-deploy verification, rollback | Failed health check triggers rollback; authorization traceable |
| M7 | Memory, factory export/import/resume | Round-trip export → import on clean `TP_FACTORY`; resume without duplicate runs |
| M8 | Daemon, Remote Control, dashboard, vault web UI, on-demand tunnel, passkeys | Tunnel down after session/idle/TTL; passkey required |
| M9 | Pricing versions, metrics, hours saved | Cost uses record's pricing version; missing telemetry reported |
| M10 | Hardening = PRD Phase 7 | Concurrency, isolation, bypass, migration, recovery suites |

## 15. Still open
1. **Pilot project** (decided later by the founder).
2. **Where the cockpit's policy lives:** user `~/.claude/settings.json` (applies to all your Claude sessions) or managed settings (machine-wide, needs admin).
3. Spike 6 (Remote Control cockpit) answered from docs only; its checks are M8 acceptance tests. Spike 5: PRF works on Android and macOS Chrome; **iOS is untested**.
