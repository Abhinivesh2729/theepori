# ADR 0001: Architecture for PRD v2

Date: 2026-10-19 · Status: accepted · Jira: SF-13

## Context

PRD v2 asks for a Tauri 2 + Rust + React desktop app, a dedicated mobile app, a persistent manager agent, a daily planning cycle and event-driven GitHub intake. The 10-17 design (v1) assumed a single Go binary, a Remote Control "cockpit" session, on-demand Cloudflare Tunnels with a passkey web page, and GitHub/Jira only through LLM steward runs. None of v1 was implemented, so changing it costs only documentation.

## Decisions

| # | Decision | Supersedes |
|---|---|---|
| N0 | Personal tool. Uses the founder's own `claude` CLI login on this Mac. Never sold; no third-party access; no API-key mode. | – |
| N1 | Rust core: Cargo workspace `tp-core` / `tpd` / `tp-cli` / `tp-proto`. Port the useful Go pieces with their tests. Go agent kept on `legacy/go-agent` (tag `go-agent-final`). | v1 "single Go binary" |
| N2 | `tpd` daemon (launchd user agent) owns all work. The Tauri app is a client. | v1 "tp daemon" (same idea, new language) |
| N3 | Claude Code only through the CLI subprocess and documented flags; no private APIs. | – |
| N4 | GitHub through the `gh` CLI, run deterministically by tpd (no LLM). Webhooks via the relay, reconciliation with ETags. | v1 D5 (gh only via Claude stewards) |
| N5 | Jira through Claude's Atlassian Rovo connector in steward runs (outbox, idempotency label, confirm-read). | v1 D4 kept in spirit |
| N6 | Manager = deterministic engine + stateless `claude -p --json-schema` proposals; state in SQLite. Remote Control optional. | v1 §2.1 cockpit as the entry point |
| N7 | Remote access through a Cloudflare Worker + Durable Object relay, E2E-encrypted, FCM content-free push. | v1 D8/D9 (on-demand tunnel + passkey page) |
| N8 | Mobile app in Flutter, Android only. | – |
| N9 | Remote vault unlock from the paired phone (Keystore + biometrics, scoped lease). | v1 §9.1 WebAuthn PRF through a tunnel |
| N10 | No agent can change policy; project config only tightens. | v1 §6 kept |
| N11 | Kept from v1: D3, D6/D7 (TP Vault replaces Panuval), D10, D11, D12 and all spike findings (`--setting-sources user`, generated `--settings`, `--strict-mcp-config`, explicit cloudflared `--config`). | – |
| N12 | Target build is this Mac only. No Linux, Windows or iOS code paths. | v1 implied macOS + Linux |
| N13 | Work tracked in Jira project SF (epic per milestone); commits `SF-<n>: …`; branches `factory/m<N>-*`; one PR per milestone gate. | – |

## Consequences

- One toolchain for desktop, daemon and CLI; shared types between Rust and the React UI.
- GitHub operations become cheap, deterministic and testable; Jira still costs a small steward run per batch.
- The relay is new infrastructure, but it is the only way to give the phone push, honest offline state and always-available approvals.
- Dropping the cockpit means tp needs its own chat surface (desktop + phone) for the manager.
- Mac-only removes cross-platform service code and test matrices until the founder asks for them.
