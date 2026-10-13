// Package agent runs the coding loop: send the conversation to the model,
// execute the tool calls it makes (through the permission engine), feed
// results back, and repeat until the model answers without tools.
package agent

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/providers"
	"github.com/Abhinivesh2729/theepori/internal/session"
	"github.com/Abhinivesh2729/theepori/internal/tools"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

// UI receives everything the user should see.
type UI interface {
	Text(delta string)
	Thinking(delta string)
	ToolStreaming(name string)
	ToolCall(req permission.Request)
	ToolResult(tool string, res tools.Result)
	Plan(steps []tools.PlanStep, note string)
	Notice(msg string)
	TurnEnd(s TurnSummary)
}

// ChangedFile is a file changed this turn. Shown means the user approved
// exactly this turn's change with a complete diff preview.
type ChangedFile struct {
	tools.FileChange
	Shown bool
}

type TurnSummary struct {
	Files     []ChangedFile
	Changed   []string
	Diff      string
	Tests     []tools.TestRecord
	Usage     llm.Usage
	Cost      float64
	Steps     int
	Stopped   string // why the turn ended early, if it did
	SessionID string
}

type Agent struct {
	Cfg     *config.Loaded
	WS      *workspace.Workspace
	Tools   *tools.Registry
	Perms   *permission.Engine
	Audit   *permission.AuditLog
	Store   *session.Store
	Session *session.Session
	Env     *tools.Env
	UI      UI

	client     *llm.Client
	notes      []string          // context to attach to the next user message
	approved   map[string]string // path → diff the user approved this turn
	lastMode   string
	turnUsage  llm.Usage
	turnCost   float64
	turnTarget llm.Target
}

// SetModel switches the model (and fallbacks) for subsequent requests.
func (a *Agent) SetModel(ref string) error {
	c, err := providers.Client(a.Cfg, ref, a.UI.Notice)
	if err != nil {
		return err
	}
	a.client = c
	a.Session.Model = ref
	return nil
}

// SetClient replaces the LLM client (tests, custom routing).
func (a *Agent) SetClient(c *llm.Client) { a.client = c }

func (a *Agent) Model() string { return a.Session.Model }

func (a *Agent) Primary() llm.Target {
	if a.client == nil || len(a.client.Targets) == 0 {
		return llm.Target{}
	}
	return a.client.Targets[0]
}

func (a *Agent) save() {
	a.Session.Tracker = a.Env.Tracker.State()
	if err := a.Store.Save(a.Session); err != nil {
		a.UI.Notice("warning: could not save session: " + err.Error())
	}
}

// Run executes one user turn.
func (a *Agent) Run(ctx context.Context, input string) error {
	if a.client == nil {
		if err := a.SetModel(a.Session.Model); err != nil {
			return err
		}
	}
	if a.Session.Title == "" {
		a.Session.Title = title(input)
	}
	a.repairHistory()
	a.Env.Tracker.StartTurn()
	a.approved = map[string]string{}
	a.turnUsage, a.turnCost = llm.Usage{}, 0
	start := time.Now()
	a.Session.Status = session.StatusActive
	a.Session.Turns++
	a.appendUser(a.decorate(input))
	a.save()

	summary := TurnSummary{SessionID: a.Session.ID}
	err := a.loop(ctx, &summary)
	if err != nil {
		a.repairHistory()
		a.Session.Status = session.StatusInterrupted
		if errors.Is(ctx.Err(), context.Canceled) {
			summary.Stopped = "interrupted — your next message continues the session"
			err = nil
		}
	} else {
		a.Session.Status = session.StatusIdle
	}
	summary.Changed = changedIn(a.Env.Tracker)
	if len(summary.Changed) > 0 {
		summary.Diff = a.Env.Tracker.Diff(summary.Changed)
		for _, fc := range a.Env.Tracker.TurnChanges(summary.Changed) {
			shown := fc.TurnDiff != "" && a.approved[fc.Path] == fc.TurnDiff
			summary.Files = append(summary.Files, ChangedFile{FileChange: fc, Shown: shown})
		}
	}
	summary.Tests = a.Env.Tracker.TestsSince(start)
	summary.Usage, summary.Cost = a.turnUsage, a.turnCost
	a.save()
	a.UI.TurnEnd(summary)
	return err
}

func changedIn(t *tools.Tracker) []string {
	all := map[string]bool{}
	for _, f := range t.Changed() {
		all[f] = true
	}
	var out []string
	for _, f := range t.TurnChanged() {
		if all[f] {
			out = append(out, f)
		}
	}
	return out
}

func (a *Agent) loop(ctx context.Context, sum *TurnSummary) error {
	max := a.Cfg.Agent.MaxSteps
	for step := 1; ; step++ {
		sum.Steps = step
		if step > max {
			sum.Stopped = fmt.Sprintf("stopped after %d steps (agent.max_steps); say \"continue\" to keep going", max)
			return nil
		}
		if err := a.maybeCompact(ctx); err != nil {
			return err
		}
		req := llm.Request{
			System:    a.Session.System,
			Messages:  a.Session.Messages,
			Tools:     a.Tools.Specs(),
			MaxTokens: a.Cfg.Agent.MaxTokens,
			Effort:    a.Cfg.EffortFor(a.Session.Model),
		}
		resp, target, err := a.client.Chat(ctx, req, a.onStream)
		if err != nil {
			return err
		}
		a.turnTarget = target
		a.account(target, resp.Usage)
		a.Session.Messages = append(a.Session.Messages, resp.Message)
		a.save()

		calls := resp.Message.ToolCalls()
		switch resp.StopReason {
		case llm.StopRefusal:
			sum.Stopped = "the model declined this request (" + strings.TrimSpace(resp.StopDetail) + "); rephrase or switch models with /model"
			if len(calls) > 0 {
				a.appendResults(cancelResults(calls, "not executed: response was declined"))
			}
			return nil
		case llm.StopMaxTokens:
			if len(calls) > 0 {
				// Inputs may be truncated: don't run them.
				a.appendResults(cancelResults(calls, "not executed: the response hit max_tokens and the tool input may be truncated. Retry with a smaller change (e.g. several edit_file calls)."))
				continue
			}
			a.appendUser("<system-reminder>Your previous reply was cut off at the output limit. Continue exactly where you stopped.</system-reminder>")
			continue
		}
		if len(calls) == 0 {
			return nil
		}
		results, err := a.execAll(ctx, calls)
		a.appendResults(results)
		a.save()
		if err != nil {
			return err
		}
	}
}

func (a *Agent) onStream(ev llm.StreamEvent) {
	switch {
	case ev.Text != "":
		a.UI.Text(ev.Text)
	case ev.Thinking != "":
		a.UI.Thinking(ev.Thinking)
	case ev.ToolName != "":
		a.UI.ToolStreaming(ev.ToolName)
	}
}

func (a *Agent) account(t llm.Target, u llm.Usage) {
	cost := llm.Cost(t.Model, u)
	a.turnUsage.Add(u)
	a.turnCost += cost
	a.Session.Usage.Add(u)
	a.Session.Cost += cost
	a.Session.LastContextTokens = u.InputTokens + u.CacheReadTokens + u.CacheWriteTokens + u.OutputTokens
}

// execAll runs tool calls sequentially (approvals are interactive). On
// cancellation the remaining calls get "canceled" results so history stays valid.
func (a *Agent) execAll(ctx context.Context, calls []llm.ToolCall) ([]llm.ToolResult, error) {
	var out []llm.ToolResult
	for i, c := range calls {
		if ctx.Err() != nil {
			out = append(out, cancelResults(calls[i:], "canceled by the user before execution")...)
			return out, ctx.Err()
		}
		out = append(out, a.exec(ctx, c))
	}
	return out, ctx.Err()
}

func (a *Agent) exec(ctx context.Context, c llm.ToolCall) llm.ToolResult {
	res := llm.ToolResult{CallID: c.ID, Name: c.Name}
	tool, ok := a.Tools.Get(c.Name)
	if !ok {
		res.Content, res.IsError = fmt.Sprintf("Error: unknown tool %q", c.Name), true
		a.UI.ToolResult(c.Name, tools.Result{IsError: true, Display: "unknown tool"})
		return res
	}
	call, err := tool.Prepare(a.Env, c.Input)
	if err != nil {
		res.Content, res.IsError = "Error: "+err.Error(), true
		a.UI.ToolResult(c.Name, tools.Result{IsError: true, Display: err.Error()})
		a.Audit.Log(permission.AuditEntry{Session: a.Session.ID, Tool: c.Name, Decision: "invalid", Error: err.Error()})
		return res
	}
	a.UI.ToolCall(call.Perm)
	outcome := a.Perms.Check(ctx, call.Perm)
	entry := permission.AuditEntry{Session: a.Session.ID, Tool: c.Name, Risk: call.Perm.Risk.String(), Summary: call.Perm.Summary, Decision: outcome.How}
	if !outcome.Allowed {
		msg := "Permission denied by the user."
		if outcome.How == "policy" {
			msg = "Blocked by policy: " + outcome.Feedback
		} else if f := strings.TrimSpace(outcome.Feedback); f != "" {
			msg = "The user denied this action and said: " + f
		}
		msg += " Do not retry the same action."
		res.Content, res.IsError = msg, true
		a.UI.ToolResult(c.Name, tools.Result{IsError: true, Display: "denied"})
		a.Audit.Log(entry)
		return res
	}
	if outcome.How == "approved" && call.Perm.Path != "" && call.Perm.Detail != "" &&
		strings.Count(call.Perm.Detail, "\n") <= tools.PreviewLines {
		if _, seen := a.approved[call.Perm.Path]; !seen {
			a.approved[call.Perm.Path] = call.Perm.Detail
		} else {
			a.approved[call.Perm.Path] = "" // several edits: the turn diff is no single preview
		}
	}
	r := call.Run(ctx)
	entry.ExitCode, entry.DurationMs = r.ExitCode, r.Duration.Milliseconds()
	if r.IsError {
		entry.Error = r.Display
	}
	a.Audit.Log(entry)
	a.UI.ToolResult(c.Name, r)
	res.Content, res.IsError = r.Output, r.IsError
	return res
}

func cancelResults(calls []llm.ToolCall, why string) []llm.ToolResult {
	var out []llm.ToolResult
	for _, c := range calls {
		out = append(out, llm.ToolResult{CallID: c.ID, Name: c.Name, Content: why, IsError: true})
	}
	return out
}

func (a *Agent) appendResults(rs []llm.ToolResult) {
	if len(rs) == 0 {
		return
	}
	m := llm.Message{Role: llm.RoleUser}
	for i := range rs {
		m.Parts = append(m.Parts, llm.Part{Type: llm.PartToolResult, ToolResult: &rs[i]})
	}
	a.Session.Messages = append(a.Session.Messages, m)
}

// appendUser adds text as a user turn. A trailing user message (tool results
// or an interrupted prompt) is extended rather than duplicated; it comes after
// every assistant turn, so earlier history stays byte-identical.
func (a *Agent) appendUser(text string) {
	n := len(a.Session.Messages)
	if n > 0 && a.Session.Messages[n-1].Role == llm.RoleUser {
		last := &a.Session.Messages[n-1]
		last.Parts = append(last.Parts, llm.Part{Type: llm.PartText, Text: text})
		return
	}
	a.Session.Messages = append(a.Session.Messages, llm.UserText(text))
}

// repairHistory closes tool calls left without results (crash or cancel).
func (a *Agent) repairHistory() {
	msgs := a.Session.Messages
	if len(msgs) == 0 || msgs[len(msgs)-1].Role != llm.RoleAssistant {
		return
	}
	if calls := msgs[len(msgs)-1].ToolCalls(); len(calls) > 0 {
		a.appendResults(cancelResults(calls, "interrupted before execution"))
	}
}

// AddNote attaches context (e.g. "files were reverted") to the next user
// message, keeping the system prompt frozen.
func (a *Agent) AddNote(note string) { a.notes = append(a.notes, note) }

// decorate adds per-turn context to the user's text (append-only).
func (a *Agent) decorate(input string) string {
	notes := a.notes
	a.notes = nil
	mode := a.Perms.Mode()
	if mode == "plan" {
		notes = append(notes, planModeNote)
	} else if a.lastMode == "plan" {
		notes = append(notes, "<system-reminder>Plan mode is off: you may now edit files and run commands (subject to approval).</system-reminder>")
	}
	a.lastMode = mode
	if a.Session.Turns <= 1 {
		if rel := workspace.Relevant(input, a.WS.Files(), 8); len(rel) > 0 {
			notes = append(notes, "<context>Files whose paths match the request (a hint, not exhaustive): "+strings.Join(rel, ", ")+"</context>")
		}
	}
	if len(notes) == 0 {
		return input
	}
	return input + "\n\n" + strings.Join(notes, "\n")
}

func title(s string) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > 60 {
		s = s[:57] + "…"
	}
	return s
}
