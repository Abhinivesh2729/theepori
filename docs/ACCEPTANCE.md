# PRD acceptance criteria — status

Build: `factory/m1-m11` · Target: this Mac (MacBookPro15,1, Intel i7-9750H, macOS 15.7.9) · Mobile (M7) deferred by the founder.

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Register repositories and map them to Jira projects | ✅ | `project.add` (`jira_key`), `tp project add --jira`; `registry` tests |
| 2 | Run and manage local Claude Code sessions without rebuilding the harness | ✅ | `runtime::Claude` drives the installed CLI (stream-json, resume, SIGINT); real run on 2026-10-19 (haiku): implement → verify → review |
| 3 | Persistent manager and work state survive restarts | ✅ | State in SQLite; `engine::recover`; test `plan_approval_releases_tasks_and_recovery_requeues`; tpd restart test |
| 4 | One consolidated daily plan for routine maintenance | ✅ | `generate_plan` at `planning.daily_plan_at`; one Plan approval; test above |
| 5 | Approve once; authorized tasks proceed without repeated prompts | ✅ | Plan approval moves all items to `approved`; scheduler assigns within limits |
| 6 | Process maintenance issues, create PRs, verify, escalate failures | ✅ | `engine::work` + `check_pr`; tests `low_risk_task_flows_to_merged_done`; bounded repairs → Escalation approval |
| 7 | Independent review + deterministic verification before merges | ✅ | `verify::run_checks` (tp-run), reviewer run (read-only), `policy::merge_gate`; real reviewer run |
| 8 | Monitor dependencies and security with approval rules | ✅ | `tp-deps` (OSV + registries, 50 tests) → `scan_deps` → Dependency approvals |
| 9 | Track CI/CD and verify deployments | ✅ | `gh` CI intake + PR checks; `deploy::deploy` health/version/rollback; test `success_and_rollback_on_failed_health` |
| 10 | Audit, usage, cost and productivity data | ✅ | Hash-chained audit; `analytics::summary` (reported cost labelled list-price estimate; hours-saved method disclosed) |
| 11 | Remote access from the mobile app | ⏸ deferred | M7 (relay + Flutter Android) postponed by the founder |
| 12 | Pause automation and intervene | ✅ | Controls (normal/paused/safe), Stop all, run stop, `task.message`, Open in Terminal |
| 13 | Export and restore memory and configuration | ✅ | `portability` round-trip test into a clean factory; memory import dedup/conflicts |
| 14 | Optional encrypted TP Vault export with explicit authorization | ✅ | `include_vault` + confirmation phrase; vault stays encrypted; validated before restore |
| 15 | Unauthorized merges and deployments blocked by deterministic policy | ✅ | `merge_gate`/`deploy_gate` tests; e2e `high_risk_change_is_never_auto_merged`, `safe_mode_blocks_merge_even_with_approval` |
| 16 | Meet measured performance targets | ⚠️ partial | `docs/perf/m11.json`: IPC p95 0.15 ms ✅, cold start 884 ms ✅, process-relaunch warm start p95 696 ms ❌ (SF-22; window reopen is instant since the app stays resident) |
| 17 | Recover from network failures, agent crashes, repeated test failures, interrupted workflows | ✅ | `recover()`, outbox retry/backoff → blocked, bounded repair loop, fail-closed hooks, deploys interrupted → escalated |

Known limits:
- **macOS privacy (TCC):** a launchd tpd cannot read repos under `~/Documents`, `~/Desktop` or `~/Downloads` until you grant **Full Disk Access** to `~/Library/Application Support/theepori/bin/tpd`. `tp doctor` and `project.add` say so instead of hanging (SF-21).
- Projects without a GitHub remote stop at *awaiting approval* with the verified change on branch `tp/<task>`.
- Jira sync needs the Atlassian connector signed in for Claude Code (`claude` → claude.ai connectors).
