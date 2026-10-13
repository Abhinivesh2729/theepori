// Package tools implements the agent's tools behind one interface. Each call
// is prepared first (validated, risk-classified, previewed) so the permission
// engine can show the exact effect before anything runs.
package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/secrets"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

// Env is what tools may touch.
type Env struct {
	WS           *workspace.Workspace
	Redact       *secrets.Redactor
	Tracker      *Tracker
	MaxFileBytes int
	TestCommand  string
	StripEnv     []string // env vars removed from child processes (API keys)
	Ask          func(ctx context.Context, question string) (string, error)
	OnPlan       func(steps []PlanStep, note string)
}

type Result struct {
	Output   string // sent to the model
	IsError  bool
	Display  string // short line for the UI
	ExitCode *int
	Duration time.Duration
}

func errResult(format string, a ...any) Result {
	msg := fmt.Sprintf(format, a...)
	return Result{Output: "Error: " + msg, IsError: true, Display: msg}
}

// Call is a validated, ready-to-run invocation.
type Call struct {
	Perm permission.Request
	Run  func(ctx context.Context) Result
}

type Tool interface {
	Name() string
	Description() string
	Schema() string
	Prepare(env *Env, input json.RawMessage) (*Call, error)
}

type Registry struct{ tools map[string]Tool }

func NewRegistry(ts ...Tool) *Registry {
	r := &Registry{tools: map[string]Tool{}}
	for _, t := range ts {
		r.tools[t.Name()] = t
	}
	return r
}

// Default returns the built-in tool set.
func Default() *Registry {
	return NewRegistry(
		ReadFile{}, ListDir{}, Glob{}, Grep{},
		WriteFile{}, EditFile{},
		RunCommand{}, RunTests{},
		GitStatus{}, GitDiff{},
		UpdatePlan{}, AskUser{},
	)
}

func (r *Registry) Get(name string) (Tool, bool) {
	t, ok := r.tools[name]
	return t, ok
}

// Sorted returns tools ordered by name (stable for prompt caching).
func (r *Registry) Sorted() []Tool {
	var out []Tool
	for _, t := range r.tools {
		out = append(out, t)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name() < out[j].Name() })
	return out
}

func (r *Registry) Specs() []llm.ToolSpec {
	var out []llm.ToolSpec
	for _, t := range r.Sorted() {
		out = append(out, llm.ToolSpec{Name: t.Name(), Description: t.Description(), Schema: json.RawMessage(t.Schema())})
	}
	return out
}

// decode parses tool input strictly enough to catch truncated or wrong JSON.
func decode(input json.RawMessage, v any) error {
	if len(input) == 0 {
		input = json.RawMessage("{}")
	}
	dec := json.NewDecoder(strings.NewReader(string(input)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return fmt.Errorf("invalid input JSON: %v", err)
	}
	return nil
}

const maxOutput = 30_000

// clip keeps the head and tail of long output.
func clip(s string, max int) string {
	if len(s) <= max {
		return s
	}
	head := max * 2 / 3
	tail := max - head
	return s[:head] + fmt.Sprintf("\n\n… [%d bytes omitted] …\n\n", len(s)-max) + s[len(s)-tail:]
}

func plural(n int, word string) string {
	if n == 1 {
		return fmt.Sprintf("%d %s", n, word)
	}
	return fmt.Sprintf("%d %ss", n, word)
}
