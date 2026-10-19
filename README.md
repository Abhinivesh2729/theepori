# Thee Pori (தீப்பொறி) — software factory

A personal software factory for one founder on one Mac. A persistent daemon (`tpd`) manages your projects. It runs your installed Claude Code CLI as its employees, and every policy, approval and audit decision is enforced in deterministic code.

> **Status: M0–M6, M8–M11 built; M7 (mobile + relay) deferred.** See [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md). The previous Go coding agent is archived on [`legacy/go-agent`](https://github.com/Abhinivesh2729/theepori/tree/legacy/go-agent).

## Try it

```bash
sh scripts/build-app.sh          # builds target/release/bundle/macos/Thee Pori.app (tp + tpd inside)
open "target/release/bundle/macos/Thee Pori.app"
```

1. Click **Start factory** in the banner. This installs `tpd` as a launchd agent in `~/Library/Application Support/theepori/bin`.
2. If your repos live in `~/Documents`, grant **Full Disk Access** to that `tpd` (System Settings → Privacy & Security → Full Disk Access → + → ⌘⇧G → paste the path). Then click **Start factory** again.
3. **Projects → Add** a local git checkout (optionally with a Jira key). tp writes `project.yaml` and a detected `verify.yaml` you can edit.
4. **Tasks → New task** (tick *approve now*), or let GitHub issues and CI failures flow in and approve the **Daily plan**.
5. Watch the agent in the **Coding Workspace**. Approve merges and deploys in the **Approval Center**. **Settings & Safety** has pause, safe mode and stop‑all.

The CLI covers the same factory: `tp doctor`, `tp project add`, `tp audit verify`, `tp daemon status`.

## How it works

- `tpd` is the factory. It runs intake (GitHub via `gh`), the daily plan, assignment and the task state machine: implement → verify ⇄ repair → independent review → PR → merge gate → deploy. It also handles Jira sync through Claude's Atlassian connector, the vault, memory, analytics and export/import.
- Employees are your installed **Claude Code** CLI, launched per task in a git worktree with tp‑generated settings: the sandbox, deny rules, a fail‑closed PreToolUse policy hook, and the tp MCP tools. Agents never push, use `gh` or see secrets.
- Every decision that matters is deterministic code with an audit record: risk, merge, deploy, budgets and policy.

## Development

```bash
cargo test --workspace
make lint
make perf                  # release perf report -> docs/perf/m0.json
```

To work on the UI in a browser against a scratch factory (never `~/factory`):

```bash
tp --factory /tmp/f setup && tpd --factory /tmp/f &
python3 scripts/dev-bridge.py --factory /tmp/f &   # 127.0.0.1:1421, proxied by Vite at /__tp
npm --prefix apps/desktop run dev                   # http://localhost:1420
```

See [AGENTS.md](AGENTS.md) for conventions. Work is tracked in Jira project **SF**.

## License

MIT
