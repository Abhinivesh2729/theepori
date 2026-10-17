# Pre-M1 spikes

Run 2026-10-17 on macOS (Darwin 24.6), Claude Code 2.1.294, signed in with a claude.ai subscription (`apiKeySource: none`). All runs used `claude-haiku-5-5`. Reproduce 1–4 with `scripts/run-spikes.sh <scratch-dir>` (read-only Jira, about $0.05 at Haiku list price). Raw outputs were kept out of the repo. Doc facts below come from code.claude.com/docs (sandboxing, hooks, headless, remote-control, scheduled-tasks, channels), fetched the same day.

| # | Question | Verdict |
|---|---|---|
| 1 | Headless `claude -p` (also under launchd) can use the claude.ai Atlassian connector | **Works** |
| 2 | A PreToolUse hook from `--settings` denies Bash and MCP calls | **Works, with one required flag** |
| 3 | The sandbox blocks agent reads of the vault directory | **Works** |
| 4 | stream-json gives usage for attribution | **Works** |
| 5 | WebAuthn PRF on iOS Safari and Android Chrome through a Cloudflare tunnel | **Pending: needs your devices** |
| 6 | A Remote Control session can run `/loop` over the tp queue and survive restarts | **Partly: docs say no for self-paced `/loop`; needs a hands-on test** |

## 1. Atlassian connector in headless and launchd runs: works

- `claude -p` loads claude.ai connectors. Tool names are `mcp__claude_ai_Atlassian__*` (e.g. `searchJiraIssuesUsingJql`, `getAccessibleAtlassianResources`).
- A read-only JQL search (`project = PAN`) returned `PAN-25` both from a shell and from a launchd job (`launchctl submit`), with no TTY and no extra env.
- At `init` the connector reports `pending` (or is missing from the list under launchd). It connects during the run and its tools are deferred: the model loads them through `ToolSearch` first. Stewards must not treat a `pending` status at init as failure.
- cloudId for 0xabhi.atlassian.net: `ef9b9055-399a-4cd4-9387-06a606b57d02`. Passing it in the prompt saves one tool call per steward run.
- The model wrapped JSON in a code fence. With `--json-schema` and `--output-format json` the result carries `structured_output: {"issue_key": "PAN-25"}`. Verified. Stewards always use it. Cost of that run: $0.0128, 6 turns.
- `--strict-mcp-config` **also removes claude.ai connectors**. Stewards that need Jira can't use it; they restrict with `--allowedTools 'mcp__claude_ai_Atlassian__*'` plus the policy hook instead.

**Design impact:** steward pattern confirmed. Add: `--json-schema` for every steward, cloudId cached in `factory.yaml`, and treat `pending` at init as normal.

## 2. Policy hook: works, but a repo can disable it unless setting sources are pinned

- A PreToolUse hook passed with `--settings <file>` denied `echo TP_DENY_ME` and `mcp__claude_ai_Atlassian__searchJiraIssuesUsingJql`, **even though both were pre-approved with `--allowedTools`**. Deny beats allow.
- Denials appear in the `result` event as `permission_denials[]` (tool name, input, tool_use_id). tp can audit them straight from the stream.
- **Critical finding:** a repository containing `.claude/settings.json` with `{"disableAllHooks": true}` **silently switched the hook off** under default setting sources. The denied command ran.
- With `--setting-sources user` the project and local settings are not loaded, and the hook was enforced again.
- `--dangerously-skip-permissions` could not be tested here (blocked by this session's own safety classifier). tp should never use it.

**Design impact (must-have):**
- Every tp-launched run uses `--setting-sources user --settings <tp-generated file>`. Repo `.claude/` settings are never loaded.
- Project instructions and skills that tp wants are injected by tp itself (`--append-system-prompt` or copied into the run's settings), not read from the repo's `.claude/`.
- The hook and sandbox also deny writes to `~/.claude/**` (user settings could otherwise disable hooks). Bash writes there were already blocked by the sandbox (`Operation not permitted`). Edit/Write outside the worktree are not auto-approved in headless mode, but tp adds explicit `Edit(~/.claude/**)` deny rules anyway.
- Doctor check: fail if the user-level `~/.claude/settings.json` contains `disableAllHooks`.

## 3. Sandbox vault isolation: works

Settings: `sandbox.enabled: true`, `autoAllowBashIfSandboxed: true`, `allowUnsandboxedCommands: false`, plus `permissions.deny: ["Read(//<vault>/**)", "Edit(//<vault>/**)"]`, with `--setting-sources user`.

| Attempt | Result |
|---|---|
| Read tool | Denied by permission rule |
| `cat` | `Operation not permitted` (macOS sandbox) |
| `python3 open()` | `PermissionError` |
| `cp` then `cat` | `cp` denied; no copy created |

The canary value never appeared in the output stream. Bash writes outside the working directory (`touch ~/.claude/...`) were also blocked.

Docs add two points:
- The sandbox covers **Bash only**. The Read/Edit/Grep/Glob tools, MCP servers and hooks run outside it, so the `permissions.deny` rules are what stop the file tools. Both layers are required. In 2.1.294 the `Read(...)` deny rule also produced a kernel-level block for `python3`, but tp also sets `sandbox.filesystem.denyRead` explicitly rather than rely on that.
- **Go CLIs such as `gh` fail TLS inside the macOS sandbox** and need `sandbox.excludedCommands`. So `gh` in steward runs executes unsandboxed. The PreToolUse hook's `gh` allow-list is then the only control on it. Coding runs keep `gh` denied.
- Useful extra: `permissions.blockReadsOutsideWorkingDirectories` for coding runs.

**Design impact:** confirmed. Use real paths (`/private/tmp`, resolved symlinks) in deny rules. Spike used a fake vault under the scratch dir; repeat against `~/factory/vault` in M4 tests.

## 4. stream-json usage: works

- `--output-format stream-json --verbose` emits `system/init` (session_id, model, tools, `mcp_servers[]` with status and source, permissionMode), `assistant`, `user` (tool results), `rate_limit_event` and a final `result`.
- `result` contains `usage` (input, output, `cache_creation_input_tokens` split 5m/1h, `cache_read_input_tokens`, thinking tokens, server tool use), `modelUsage` per model (tokens, `costUSD`, `costBasis`), `total_cost_usd`, `num_turns`, `duration_ms`, `session_id`, `permission_denials`, `subagent_stats`.
- **On a subscription, cost is notional:** `costBasis: "list"` means list price, not a bill. tp must label it "estimated at list price" and keep its own versioned pricing table (PRD §14).

Baseline overhead per run (cache creation on a trivial prompt):

| Flags | Tools | Baseline tokens | Est. cost |
|---|---|---|---|
| default (user config, all connectors) | 90 | 38,451 | $0.0077 |
| `--strict-mcp-config --setting-sources user` | 29 | 12,229 | $0.0027 |
| `--bare` | 3 | n/a: needs an API key, not usable with subscription login | |

**Design impact:** coding runs use `--strict-mcp-config` plus tp's own MCP config (tp mcp only), and stewards keep connectors but restrict tools. A Jira steward round trip (do + confirm) costs about 2 × $0.008–0.01 with Haiku.

## 5. WebAuthn PRF through a tunnel: pending

Test page ready: [`prf-test/index.html`](prf-test/index.html). Steps:

1. `python3 -m http.server 8787 --directory docs/spikes/prf-test` (local only).
2. `cloudflared tunnel --url http://localhost:8787` (quick tunnel, random `*.trycloudflare.com` host, no account).
3. On iPhone Safari and Android Chrome: open the link, tap Register, then Authenticate twice. Pass = PRF returns 32 bytes with the same fingerprint both times.
4. Stop both processes. The test passkey is bound to the random host and is useless afterwards; delete it from the device's password manager.

## 6. Remote Control cockpit: partly answered by docs, hands-on test pending

What the docs say (`claude remote-control`, scheduled tasks, channels):

| Topic | Fact | Impact on the cockpit design |
|---|---|---|
| `--settings` | **Not forwarded** to Remote Control server sessions | The cockpit's hooks and deny rules must live in a settings **file** that loads anyway: user `~/.claude/settings.json` (affects every Claude session of yours) or managed settings (admin, machine-wide). Decision needed. |
| Self-paced `/loop` | **Not restored** after a restart. `/loop` is not on the list of commands documented as working remotely | Don't depend on `/loop` for the cockpit. |
| Fixed-interval cron tasks | Restored on `--continue`/`--resume`. **All recurring tasks expire after 7 days** | Cockpit polls the tp queue with a fixed-interval task (e.g. every 10 min). `tp` re-arms it before expiry; `tp factory doctor` warns when it is missing. |
| Session restore | `claude remote-control --continue` restores the server's starting session, for **about 4 hours** after the server stops | The daemon restarts the server quickly (launchd `KeepAlive`). After a longer outage it starts a fresh cockpit, and tp re-issues pending links there. |
| Unattended start | Not documented as supported. The first run asks `Enable Remote Control? (y/n)`, and an untrusted directory with no TTY exits. The server exits after ~10 min without network | `tp setup` does the first run interactively in `~/factory` (accept + trust), then hands it to launchd with `KeepAlive`. |
| Push into a running session | **Channels** (research preview): an MCP server sends `notifications/claude/channel` into a session started with `--channels`. Sender allow-list; custom channels need allow-listing. Whether this combines with Remote Control is undocumented. Also: `claude --resume <id> "<prompt>"` sends a prompt to a running `--bg` session | Possible upgrade later: tp pushes "new task" and "unlock needed" events into the cockpit instead of the cockpit polling. Not in the core design until tested. |

Hands-on test (needs your phone and claude.ai):
1. In `~/factory` (or a scratch dir): `claude remote-control --name tp-cockpit-spike`, accept the prompt, trust the folder.
2. From the phone: open the session and run `/loop 10m run: date`. Check that it fires.
3. Stop the server, run `claude remote-control --continue`, and check whether the 10-minute task comes back.
4. Under launchd: confirm it starts without a TTY after the one-time interactive accept.
