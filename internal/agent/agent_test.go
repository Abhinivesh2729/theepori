package agent

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/tools"
)

// fakeModel replays scripted assistant turns and records requests.
type fakeModel struct {
	mu    sync.Mutex
	turns []llm.Message
	reqs  []llm.Request
	hook  func(i int, req llm.Request)
}

func (f *fakeModel) Name() string                                        { return "fake" }
func (f *fakeModel) ListModels(context.Context) ([]llm.ModelInfo, error) { return nil, nil }
func (f *fakeModel) Chat(ctx context.Context, req llm.Request, on llm.StreamFunc) (*llm.Response, error) {
	f.mu.Lock()
	i := len(f.reqs)
	f.reqs = append(f.reqs, req)
	f.mu.Unlock()
	if f.hook != nil {
		f.hook(i, req)
	}
	if ctx.Err() != nil {
		return nil, llm.FromTransport("fake", "", ctx.Err())
	}
	m := f.turns[min(i, len(f.turns)-1)]
	if on != nil {
		on(llm.StreamEvent{Text: m.Text()})
	}
	stop := llm.StopEnd
	if len(m.ToolCalls()) > 0 {
		stop = llm.StopToolUse
	}
	return &llm.Response{Message: m, StopReason: stop, Usage: llm.Usage{InputTokens: 100, OutputTokens: 10}}, nil
}

func call(id, name string, input any) llm.Part {
	b, _ := json.Marshal(input)
	return llm.Part{Type: llm.PartToolCall, ToolCall: &llm.ToolCall{ID: id, Name: name, Input: b}}
}

func asst(parts ...llm.Part) llm.Message { return llm.Message{Role: llm.RoleAssistant, Parts: parts} }
func text(s string) llm.Part             { return llm.Part{Type: llm.PartText, Text: s} }

type recUI struct {
	mu      sync.Mutex
	calls   []string
	results []string
	plans   int
	end     TurnSummary
}

func (u *recUI) Text(string)          {}
func (u *recUI) Thinking(string)      {}
func (u *recUI) ToolStreaming(string) {}
func (u *recUI) Notice(string)        {}
func (u *recUI) ToolCall(r permission.Request) {
	u.mu.Lock()
	u.calls = append(u.calls, r.Tool)
	u.mu.Unlock()
}
func (u *recUI) ToolResult(tool string, r tools.Result) {
	u.mu.Lock()
	u.results = append(u.results, tool+":"+r.Display)
	u.mu.Unlock()
}
func (u *recUI) Plan([]tools.PlanStep, string) { u.plans++ }
func (u *recUI) TurnEnd(s TurnSummary)         { u.end = s }

type approver struct {
	allow bool
	fn    func(permission.Request)
}

func (a approver) Approve(_ context.Context, r permission.Request) (permission.Answer, error) {
	if a.fn != nil {
		a.fn(r)
	}
	return permission.Answer{Allow: a.allow, Feedback: map[bool]string{false: "use a constant instead"}[a.allow]}, nil
}

func repo(t *testing.T) string {
	t.Helper()
	t.Setenv("TP_HOME", t.TempDir())
	t.Setenv("ANTHROPIC_API_KEY", "sk-ant-test-dummy-key-0000000000")
	t.Setenv("TP_MODEL", "")
	t.Setenv("TP_CREDENTIAL_STORE", "file")
	dir := t.TempDir()
	dir, _ = filepath.EvalSymlinks(dir)
	os.WriteFile(filepath.Join(dir, "calc.sh"), []byte("add() { echo $(( $1 + $2 )); }\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "test.sh"), []byte(". ./calc.sh\n[ \"$(add 2 2)\" = 4 ] && echo '1 passed' || { echo '1 failed'; exit 1; }\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "AGENTS.md"), []byte("Run tests with: sh test.sh\n"), 0o644)
	os.MkdirAll(filepath.Join(dir, ".theepori"), 0o755)
	os.WriteFile(filepath.Join(dir, ".theepori", "config.yaml"), []byte("test_command: sh test.sh\n"), 0o644)
	exec.Command("git", "-C", dir, "init", "-q").Run()
	return dir
}

func newTestAgent(t *testing.T, dir string, fake *fakeModel, mode string, ap permission.Approver, resume string) (*Agent, *recUI) {
	t.Helper()
	ui := &recUI{}
	a, err := New(Options{Cwd: dir, Mode: mode, UI: ui, Approver: ap, Resume: resume})
	if err != nil {
		t.Fatal(err)
	}
	a.SetClient(&llm.Client{Targets: []llm.Target{{Ref: "fake/m", Provider: fake, Model: llm.ModelInfo{ID: "m", ContextWindow: 100_000, InputPrice: 1, OutputPrice: 2}}}})
	return a, ui
}

// The core loop: Understand → Plan → Modify → Test → Review.
func TestAgentEndToEnd(t *testing.T) {
	dir := repo(t)
	// Introduce a bug for the agent to fix.
	os.WriteFile(filepath.Join(dir, "calc.sh"), []byte("add() { echo $(( $1 - $2 )); }\n"), 0o644)
	fake := &fakeModel{turns: []llm.Message{
		asst(text("Looking."), call("1", "update_plan", map[string]any{"steps": []map[string]string{{"step": "fix add", "status": "in_progress"}, {"step": "run tests", "status": "pending"}}}), call("2", "read_file", map[string]any{"path": "calc.sh"})),
		asst(call("3", "edit_file", map[string]any{"path": "calc.sh", "old_string": "$1 - $2", "new_string": "$1 + $2"})),
		asst(call("4", "run_tests", map[string]any{})),
		asst(text("Fixed add(); tests pass.")),
	}}
	var asked []string
	a, ui := newTestAgent(t, dir, fake, "default", approver{allow: true, fn: func(r permission.Request) { asked = append(asked, r.Tool) }}, "")
	if err := a.Run(context.Background(), "the add function in calc.sh is broken, fix it"); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(filepath.Join(dir, "calc.sh"))
	if !strings.Contains(string(b), "$1 + $2") {
		t.Fatalf("file not fixed: %s", b)
	}
	if strings.Join(asked, ",") != "edit_file,run_tests" {
		t.Fatalf("approvals asked for %v (reads must not ask)", asked)
	}
	if len(ui.end.Tests) != 1 || !ui.end.Tests[0].Passed || len(ui.end.Changed) != 1 || !strings.Contains(ui.end.Diff, "+add() { echo $(( $1 + $2 )); }") {
		t.Fatalf("summary = %+v", ui.end)
	}
	if ui.plans != 1 || ui.end.Cost <= 0 || ui.end.Steps != 4 {
		t.Fatalf("plans=%d cost=%v steps=%d", ui.plans, ui.end.Cost, ui.end.Steps)
	}
	// System prompt carries project instructions; first user message has a relevance hint.
	if !strings.Contains(fake.reqs[0].System, "sh test.sh") || !strings.Contains(fake.reqs[0].Messages[0].Text(), "calc.sh") {
		t.Fatal("context engine output missing")
	}
	// Tool results flow back paired with their calls.
	last := fake.reqs[1].Messages[len(fake.reqs[1].Messages)-1]
	if last.Role != llm.RoleUser || len(last.Parts) != 2 || last.Parts[1].ToolResult.CallID != "2" || !strings.Contains(last.Parts[1].ToolResult.Content, "add()") {
		t.Fatalf("tool results = %+v", last)
	}

	// Session persisted and resumable with a byte-identical system prompt.
	id := a.Session.ID
	fake2 := &fakeModel{turns: []llm.Message{asst(text("Nothing else to do."))}}
	b2, _ := newTestAgent(t, dir, fake2, "", approver{allow: true}, id)
	if b2.Session.ID != id || len(b2.Session.Messages) != 8 {
		t.Fatalf("resumed %s with %d messages", b2.Session.ID, len(b2.Session.Messages))
	}
	if err := b2.Run(context.Background(), "anything else?"); err != nil {
		t.Fatal(err)
	}
	if fake2.reqs[0].System != fake.reqs[0].System || len(fake2.reqs[0].Messages) != 9 {
		t.Fatal("resume must replay identical system prompt and full history")
	}
	audit, _ := os.ReadFile(a.Store.AuditPath())
	if strings.Count(string(audit), "\n") < 4 || !strings.Contains(string(audit), `"decision":"approved"`) {
		t.Fatalf("audit log:\n%s", audit)
	}
}

func TestDenialFeedbackAndPlanMode(t *testing.T) {
	dir := repo(t)
	fake := &fakeModel{turns: []llm.Message{
		asst(call("1", "edit_file", map[string]any{"path": "calc.sh", "old_string": "$1 + $2", "new_string": "$1 * $2"})),
		asst(text("OK, I won't change it.")),
	}}
	a, _ := newTestAgent(t, dir, fake, "default", approver{allow: false}, "")
	if err := a.Run(context.Background(), "multiply instead"); err != nil {
		t.Fatal(err)
	}
	res := fake.reqs[1].Messages[2].Parts[0].ToolResult
	if !res.IsError || !strings.Contains(res.Content, "use a constant instead") {
		t.Fatalf("denial result = %+v", res)
	}
	b, _ := os.ReadFile(filepath.Join(dir, "calc.sh"))
	if !strings.Contains(string(b), "$1 + $2") {
		t.Fatal("denied edit was applied")
	}

	// Plan mode blocks writes by policy without asking, and tells the model.
	fake = &fakeModel{turns: []llm.Message{
		asst(call("1", "write_file", map[string]any{"path": "x.txt", "content": "x"})),
		asst(text("Plan: ...")),
	}}
	asked := false
	a, _ = newTestAgent(t, dir, fake, "plan", approver{allow: true, fn: func(permission.Request) { asked = true }}, "")
	if err := a.Run(context.Background(), "plan something"); err != nil {
		t.Fatal(err)
	}
	if asked || !strings.Contains(fake.reqs[0].Messages[0].Text(), "PLAN MODE") {
		t.Fatal("plan mode must not ask and must inform the model")
	}
	if _, err := os.Stat(filepath.Join(dir, "x.txt")); err == nil {
		t.Fatal("plan mode wrote a file")
	}
}

func TestInterruptKeepsHistoryValid(t *testing.T) {
	dir := repo(t)
	ctx, cancel := context.WithCancel(context.Background())
	fake := &fakeModel{turns: []llm.Message{
		asst(call("1", "run_command", map[string]any{"command": "touch a"}), call("2", "run_command", map[string]any{"command": "touch b"})),
	}}
	a, ui := newTestAgent(t, dir, fake, "default", approver{allow: false, fn: func(permission.Request) { cancel() }}, "")
	if err := a.Run(ctx, "do things"); err != nil {
		t.Fatal(err)
	}
	if a.Session.Status != "interrupted" || !strings.Contains(ui.end.Stopped, "interrupted") {
		t.Fatalf("status=%s stopped=%q", a.Session.Status, ui.end.Stopped)
	}
	msgs := a.Session.Messages
	last := msgs[len(msgs)-1]
	if last.Role != llm.RoleUser || len(last.Parts) != 2 {
		t.Fatalf("every tool call needs a result: %+v", last)
	}
	// Next input merges into the trailing user message (no user,user pair).
	fake.turns = []llm.Message{asst(text("continuing"))}
	if err := a.Run(context.Background(), "continue"); err != nil {
		t.Fatal(err)
	}
	for i := 1; i < len(a.Session.Messages); i++ {
		if a.Session.Messages[i].Role == a.Session.Messages[i-1].Role {
			t.Fatalf("consecutive %s messages at %d", a.Session.Messages[i].Role, i)
		}
	}
}

func TestCompaction(t *testing.T) {
	dir := repo(t)
	fake := &fakeModel{turns: []llm.Message{asst(text("first answer"))}}
	a, _ := newTestAgent(t, dir, fake, "default", approver{allow: true}, "")
	if err := a.Run(context.Background(), "hello"); err != nil {
		t.Fatal(err)
	}
	a.Session.LastContextTokens = 90_000 // > 80% of 100k
	fake.turns = []llm.Message{asst(text("SUMMARY: user said hello")), asst(text("second answer"))}
	fake.reqs = nil
	if err := a.Run(context.Background(), "next question"); err != nil {
		t.Fatal(err)
	}
	if len(fake.reqs) != 2 || !strings.Contains(fake.reqs[0].Messages[0].Text(), "<transcript>") {
		t.Fatalf("expected a summarisation request, got %d requests", len(fake.reqs))
	}
	msgs := fake.reqs[1].Messages
	if len(msgs) != 1 || !msgs[0].Summary || !strings.Contains(msgs[0].Text(), "SUMMARY") || !strings.Contains(msgs[0].Text(), "next question") {
		t.Fatalf("compacted history = %+v", msgs)
	}
	if a.Session.Compactions != 1 {
		t.Fatal("compaction not counted")
	}
}

// The turn summary may only skip a diff the user approved verbatim.
func TestTurnSummaryShownOnlyForApprovedPreview(t *testing.T) {
	dir := repo(t)
	edit := func(id, from, to string) llm.Part {
		return call(id, "edit_file", map[string]any{"path": "calc.sh", "old_string": from, "new_string": to})
	}
	// One approved edit → shown.
	fake := &fakeModel{turns: []llm.Message{asst(edit("1", "$1 + $2", "$1 + $2 + 0")), asst(text("done"))}}
	a, ui := newTestAgent(t, dir, fake, "default", approver{allow: true}, "")
	if err := a.Run(context.Background(), "edit"); err != nil {
		t.Fatal(err)
	}
	if len(ui.end.Files) != 1 || !ui.end.Files[0].Shown || ui.end.Files[0].Add != 1 || ui.end.Files[0].Del != 1 {
		t.Fatalf("files = %+v", ui.end.Files)
	}
	// Approved edit, then the file is changed again by a command → not shown.
	fake.turns = []llm.Message{
		asst(edit("2", "+ 0", "+ 1")),
		asst(call("3", "run_command", map[string]any{"command": "printf '# x\\n' >> calc.sh"})),
		asst(text("done")),
	}
	fake.reqs = nil
	if err := a.Run(context.Background(), "again"); err != nil {
		t.Fatal(err)
	}
	if len(ui.end.Files) != 1 || ui.end.Files[0].Shown {
		t.Fatalf("command-modified file must show its full diff: %+v", ui.end.Files)
	}
	// Auto-approved (accept-edits) → never previewed → not shown.
	_ = a.SetMode("accept-edits")
	fake.turns = []llm.Message{asst(edit("4", "+ 1", "+ 2")), asst(text("done"))}
	fake.reqs = nil
	if err := a.Run(context.Background(), "third"); err != nil {
		t.Fatal(err)
	}
	if ui.end.Files[0].Shown {
		t.Fatal("auto-approved edits were never previewed")
	}
}
