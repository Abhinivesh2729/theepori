# AGENTS.md — Thee Pori (tp)

Go 1.27+ CLI. Module `github.com/Abhinivesh2729/theepori`.

## Commands
- Build: `make build` (→ `bin/tp`)
- Test: `go test ./...`
- Lint: `make vet` (go vet + gofmt check)

## Conventions
- Provider-specific logic stays in `internal/llm/<provider>`; the agent only sees `llm.Message`/`llm.Part`.
- History is append-only and the system prompt is frozen per session — never edit earlier messages (prompt caching + Claude thinking-block validity). Add per-turn context with `Agent.AddNote`.
- Every tool validates input in `Prepare` and returns a `permission.Request` with an accurate risk; side effects only in `Run`.
- Project config is untrusted: it may only tighten permissions.
- Errors shown to users must say how to fix them.
- Never log or persist secrets; route output through `secrets.Redactor`.
