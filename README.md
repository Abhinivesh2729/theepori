# Thee Pori (தீப்பொறி) — software factory

A personal software factory for one founder on one Mac. A persistent daemon (`tpd`) manages your projects. It runs your installed Claude Code CLI as its employees, and every policy, approval and audit decision is enforced in deterministic code.

> **Status: M0 (foundation) in progress.** The previous Go coding agent (v0.1.0) is archived on the [`legacy/go-agent`](https://github.com/Abhinivesh2729/theepori/tree/legacy/go-agent) branch (tag `go-agent-final`), and its releases still install with the existing `install.sh`.

## What exists today (M0)

- `tpd`: the factory daemon. It holds a single‑instance lock, serves JSON‑RPC on a Unix socket, pushes live events, and runs as a launchd agent that restarts on crash.
- `tp`: the CLI, with `setup`, `doctor`, `project add|list`, `daemon install|uninstall|status|run|stop|bench`, and `audit tail|verify`.
- A factory under `~/factory` (or `$TP_FACTORY`): `factory.yaml`, a SQLite state database, and a hash‑chained audit log in `audit/*.jsonl`.
- **Thee Pori.app** (Tauri 2 + React): live daemon status, projects and audit feed. Closing it never stops the factory.

What's coming next is in [docs/FACTORY_DESIGN.md](docs/FACTORY_DESIGN.md) §12: Claude Code runtime and policy hooks, Mission Control, GitHub/Jira, the manager and daily plan, verification, the vault, the Android app with relay, and more.

## Build and run (macOS)

```bash
make build                 # target/release/tp, target/release/tpd
target/release/tp setup
make install-daemon        # tp -> ~/.local/bin, tpd -> launchd
tp doctor
tp project add ~/code/my-app --jira SF
make desktop               # target/release/bundle/macos/Thee Pori.app
```

Requirements: Rust (stable), Node 24, the Claude Code CLI, `gh` and git.

## Development

```bash
cargo test --workspace
make lint
make perf                  # release perf report -> docs/perf/m0.json
```

See [AGENTS.md](AGENTS.md) for conventions. Work is tracked in Jira project **SF**.

## License

MIT
