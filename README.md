# Thee Pori (தீப்பொறி) — `tp`

A local-first AI coding agent for your terminal. One Go binary, any model (Claude, OpenAI, OpenAI-compatible endpoints, or an offline model running on your phone with [Thinai](https://play.google.com/store/apps/details?id=in.atmega.thinai)), with developer-controlled execution. No server.

```
Understand → Plan → Modify → Test → Review
```

## Install

```bash
make install            # builds bin/tp and installs to ~/.local/bin
make dist               # cross-compiles darwin/linux × amd64/arm64 into dist/
```

Requires Go 1.27+ to build. Runs on macOS and Linux.

## Quick start

```bash
export ANTHROPIC_API_KEY=sk-ant-...     # and/or OPENAI_API_KEY
cd your-repo
tp init                                 # .theepori/config.yaml (+ global config on first run)
tp doctor                               # verify config, keys, connectivity
tp                                      # interactive session
tp "add input validation to the signup handler"   # one-shot task
tp plan "migrate the cache layer to Redis"        # read-only plan
tp review --base main                             # review a branch
```

## Commands

| Command | What it does |
|---|---|
| `tp` | Interactive session (REPL). `-c` resumes the latest session, `-r <id>` a specific one |
| `tp "task"` | Run one task and exit. Reads the task from stdin when piped |
| `tp init` | Create project config; detects the test command |
| `tp config [show\|get\|set\|unset\|edit\|path]` | Global settings, or `--project` |
| `tp providers [list\|presets\|add\|remove\|test\|discover]` | Manage LLM providers; `add <preset>` needs no flags; `discover` finds Thinai on your Wi-Fi |
| `tp models [use <provider/model>]` | List models (`--remote` for live lists) and set the default |
| `tp plan "task"` | Investigate read-only and propose a plan |
| `tp session [list\|resume\|show\|delete]` | Saved sessions for this project |
| `tp review [--staged\|--base ref]` | Review changes (read-only agent) |
| `tp tools` | Tools and their risk levels |
| `tp auth [check]` | Credential status (env vars only) |
| `tp doctor [--offline]` | Diagnose configuration and connectivity |
| `tp memory [add\|clear\|path]` | Project memory included in new sessions |
| `tp audit [-n N]` | Local audit log of tool actions |
| `tp data [purge]` | Inspect or delete stored data |
| `tp version` | Build info |

Inside the REPL: `/model`, `/mode`, `/plan`, `/review`, `/diff`, `/undo`, `/compact`, `/status`, `/remember`, `/sessions`, `/resume`, `/new`, `/help`. Ctrl-C interrupts the running turn; your next message continues it. Multi-line input: end a line with `\` or wrap text in `"""`.

## Providers and models

Models are written as `provider/model`, for example `anthropic/claude-opus-5-5`, `openai/gpt-5`, or `thinai/loaded`.

| Type | Protocol | Notes |
|---|---|---|
| `anthropic` | Claude Messages API (official Go SDK) | Adaptive thinking with effort control, prompt caching, thinking blocks replayed verbatim, server-side refusal fallback |
| `openai` | Chat Completions | `max_completion_tokens` |
| `openai-compatible` | Chat Completions | Groq, OpenRouter, vLLM, LM Studio and similar; `base_url` is required |
| `thinai` / `ollama` | Ollama `/api/chat` | Thinai phone app or local Ollama; `loaded` uses whichever model is already resident |

**Presets** cover Anthropic, OpenAI, Gemini, Groq, OpenRouter, DeepSeek, Mistral, xAI, Together, Fireworks, Cerebras, Ollama, LM Studio and Thinai. A preset works as soon as its key is set:

```bash
export GROQ_API_KEY=...
tp -m groq/llama-3.3-70b-versatile "explain this repo"   # no setup needed
tp providers presets                                    # all presets + key status
tp providers add groq                                   # save it (live-tested)
tp providers add work-ai --preset openai --api-key-env WORK_OPENAI_KEY
tp providers add vllm --type openai-compatible --base-url http://gpu-box:8000/v1
tp providers discover --save                            # Thinai: Server tab → Start → Share on local network
tp models use groq/llama-3.3-70b-versatile
tp config set fallback_models "[openai/gpt-5]"
```

Each model has a **tool mode**. Large models use native tool calling. Small local models default to `text` mode: tools are described in the prompt and calls are parsed from `<tool_call>` blocks. Override per model with `providers.<name>.models.<id>.tools: native|text`.

Be realistic about small models: phone-sized models (1–4B) handle short, single-file tasks. Multi-step agentic work needs a frontier model. Use `fallback_models` to route around failures.

Reliability: transient errors (429, 5xx, timeouts) are retried with backoff and honour `retry-after`. Fallback models are tried after retries are exhausted. Every error comes with a fix (`→ export ANTHROPIC_API_KEY=...`). Token usage and estimated cost are shown per turn and per session (`/status`).

## Permissions

| Mode | Reads | File edits | Commands | Destructive |
|---|---|---|---|---|
| `plan` | ✔ | ✗ | ✗ | ✗ |
| `default` | ✔ | ask | ask | ask |
| `accept-edits` | ✔ | ✔ | ask | ask |
| `auto` (`--yes`) | ✔ | ✔ | ✔ | ask |

- Every edit is previewed as a diff before you approve it. Answer `y`, `a` (always, for this session), `n`, or type feedback, which goes back to the agent.
- Commands are classified. Read-only ones (`ls`, `git status`, `grep`, …) run freely. Destructive ones (`rm -rf`, `git push`, `reset --hard`, deploys, `terraform apply`, `DROP TABLE`, `curl | sh`, credential changes, …) always ask: you must type `yes`, and the approval is never remembered.
- Non-interactive runs fail closed: anything that needs approval is denied.
- `permissions.allow_commands` / `deny_commands` (globs) and `deny_paths` refine the policy.
- Every action and decision is written to a local audit log (`tp audit`).

## Security model

- **Credentials live in environment variables only.** tp never stores them. They are masked in output, redacted from sessions and logs, and **removed from the environment of commands the agent runs**.
- **Secrets files are off-limits.** The agent cannot read, write, list or grep `.env*`, keys, certificates, cloud credential files or `*.tfstate`.
- `git commit` is blocked if staged files or diffs look like they contain credentials.
- Paths are confined to the project root, and symlink escapes are rejected.
- **Project config is untrusted** because it arrives with a cloned repo. It can only *tighten* permissions and cannot define providers, since a repo-supplied `base_url` could redirect your API key.

## Context, sessions and memory

- Instructions are read from `AGENTS.md`, `CLAUDE.md`, `.cursorrules` and `.github/copilot-instructions.md`, at the repo root and in each directory down to your working directory.
- File listing respects `.gitignore` and `context.ignore`. The first prompt of a session gets a hint listing relevant files.
- The system prompt is frozen when a session starts and history is append-only. That keeps prompt caching effective and Claude's thinking blocks valid. Per-turn context, such as a mode change or reverted files, is appended to your next message.
- When the context reaches `agent.compact_at` (default 80%), the conversation is summarised and the history is replaced by the summary. Run `/compact` to do it by hand.
- Sessions are saved after every step, so a crash or Ctrl-C loses nothing. Resume with `tp -c`.

Data locations: `~/Library/Application Support/theepori` on macOS, `~/.config/theepori` on Linux; `TP_HOME` overrides both. The folder holds `config.yaml` and `projects/<project>/{sessions/, memory.md, audit.jsonl}`.

## Configuration

```yaml
# global: tp config path
default_model: anthropic/claude-opus-5-5
fallback_models: [openai/gpt-5]
providers:
  thinai: {type: thinai, base_url: "http://192.168.1.36:11434", num_ctx: 8192}
permissions:
  mode: default
  allow_commands: ["go test *", "npm run lint"]
agent:
  effort: medium        # Claude: low | medium | high | xhigh | max
  max_steps: 60
  compact_at: 0.8
```

Unknown keys and invalid values are rejected. The error names every problem and how to fix it.

## Architecture

```
cmd/tp                 entrypoint
internal/cli           cobra commands, REPL, slash commands
internal/agent         reasoning loop, system prompt, compaction, review
internal/llm           provider-neutral types, retries/fallback, cost, text-tool adapter
  anthropic/ openai/ ollama/    provider adapters (+ Thinai discovery)
internal/providers     builds providers/clients from config
internal/workspace     repo detection, file listing, instructions, relevance, sensitive files
internal/tools         read/list/glob/grep/write/edit/run_command/run_tests/git/plan/ask + change tracker
internal/permission    policy engine, command classifier, audit log
internal/session       session store, project memory
internal/config        layered config, validation, comment-preserving edits
internal/secrets       redaction
internal/ui            terminal rendering and approval prompts
```

## Development

```bash
make test      # unit + end-to-end agent tests (scripted model, mock provider servers)
make vet
```

## Status

The MVP covers phases 1–3 of the PRD. Not built yet:

- **Phase 4:** GitHub/GitLab, Jira, deployment providers, MCP.
- **Phase 5:** cost-based model routing, installers.
- **REPL limits:** input is line-based, with no arrow-key history or full-screen TUI.
