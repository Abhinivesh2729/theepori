# AGENTS.md — Thee Pori software factory

Rust workspace + Tauri 2 desktop app. **macOS only** (see `docs/adr/0001-prd-v2-architecture.md`).
Design: `docs/FACTORY_DESIGN.md`. Work is tracked in Jira project **SF**; commit messages start with `SF-<n>:`.

## Layout
- `crates/tp-core` — all factory logic (paths, config, db, registry, audit, redaction, doctor, launchd, client)
- `crates/tp-proto` — wire types for the tpd socket (and later relay/mobile)
- `crates/tpd` — the daemon; owns all work; Unix socket JSON-RPC
- `crates/tp-cli` — the `tp` command
- `apps/desktop` — Tauri 2 + React/TS; a client of tpd
- `scripts/perf.py` — release-build perf harness; results in `docs/perf/`

## Commands
- Test: `cargo test --workspace`
- Lint: `make lint` (fmt, clippy -D warnings, tsc)
- Build: `make build` (tp, tpd) · `make desktop` (Thee Pori.app)
- Perf: `make perf`

## Conventions
- Policy, permissions and budgets are enforced by deterministic code, never by an LLM. Project config may only tighten policy.
- Every consequential action is an audit event (`tp_core::audit`); tag agent statements as `claimed`.
- Never log, audit or persist secrets; route text through `tp_core::redact::Redactor`.
- SQLite migrations are append-only (`tp_core::db::MIGRATIONS`).
- No blocking work on the desktop UI thread; socket IO runs on worker threads.
- Errors shown to users say how to fix them.
- Tests use scratch factories (`--factory` / `TP_FACTORY`), never `~/factory`.
