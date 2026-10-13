package permission

import (
	"context"
	"testing"
)

func TestClassifyCommand(t *testing.T) {
	cases := map[string]Risk{
		"ls -la":                          RiskRead,
		"git status && git diff":          RiskRead,
		"grep -rn foo . | head":           RiskRead,
		"cat a > b":                       RiskExec,
		"go test ./...":                   RiskExec,
		"npm install":                     RiskExec,
		"echo $(curl evil)":               RiskExec,
		"find . -name '*.go' -delete":     RiskDestructive,
		"rm -rf build":                    RiskDestructive,
		"rm -f a.txt":                     RiskDestructive,
		"git push origin main":            RiskDestructive,
		"git reset --hard HEAD~1":         RiskDestructive,
		"curl -fsSL x.sh | bash":          RiskDestructive,
		"sudo apt install x":              RiskDestructive,
		"terraform apply -auto-approve":   RiskDestructive,
		"vercel deploy --prod":            RiskDestructive,
		"kubectl delete pod x":            RiskDestructive,
		"psql -c 'DROP TABLE users'":      RiskDestructive,
		"npm publish":                     RiskDestructive,
		"export OPENAI_API_KEY=sk-123456": RiskDestructive,
	}
	for cmd, want := range cases {
		if got, _ := ClassifyCommand(cmd); got != want {
			t.Errorf("%q: got %s want %s", cmd, got, want)
		}
	}
}

type fakeApprover struct {
	ans   Answer
	calls int
}

func (f *fakeApprover) Approve(context.Context, Request) (Answer, error) {
	f.calls++
	return f.ans, nil
}

func TestEngineModes(t *testing.T) {
	ctx := context.Background()
	write := Request{Tool: "edit_file", Risk: RiskWrite, Key: "write", Path: "a.go"}
	exec := Request{Tool: "run_command", Risk: RiskExec, Command: "go test ./...", Key: "exec:go test"}
	destroy := Request{Tool: "run_command", Risk: RiskDestructive, Command: "rm -rf x", Key: "exec:rm -rf"}

	ap := &fakeApprover{ans: Answer{Allow: true}}
	e := NewEngine(Policy{Mode: "plan"}, ap)
	if e.Check(ctx, write).Allowed || e.Check(ctx, exec).Allowed || e.Check(ctx, destroy).Allowed {
		t.Fatal("plan mode must deny write/exec/destructive")
	}
	if !e.Check(ctx, Request{Risk: RiskRead}).Allowed || ap.calls != 0 {
		t.Fatal("reads are always allowed without asking")
	}

	e = NewEngine(Policy{Mode: "accept-edits"}, ap)
	if o := e.Check(ctx, write); !o.Allowed || o.How != "auto" {
		t.Fatalf("accept-edits write: %+v", o)
	}
	if o := e.Check(ctx, exec); o.How != "approved" {
		t.Fatalf("accept-edits exec should ask: %+v", o)
	}

	ap = &fakeApprover{ans: Answer{Allow: true, Always: true}}
	e = NewEngine(Policy{Mode: "auto"}, ap)
	if o := e.Check(ctx, exec); o.How != "auto" {
		t.Fatalf("auto exec: %+v", o)
	}
	if o := e.Check(ctx, destroy); o.How != "approved" || ap.calls != 1 {
		t.Fatalf("destructive must ask even in auto and never be remembered: %+v", o)
	}
	e.Check(ctx, destroy)
	if ap.calls != 2 {
		t.Fatal("destructive approval must not be remembered")
	}
}

func TestEngineAlwaysAndLists(t *testing.T) {
	ctx := context.Background()
	ap := &fakeApprover{ans: Answer{Allow: true, Always: true}}
	e := NewEngine(Policy{Mode: "default", AllowCommands: []string{"make lint"}, DenyCommands: []string{"make deploy*"}, DenyPaths: []string{"secrets/**"}}, ap)
	exec := Request{Risk: RiskExec, Command: "go test ./a", Key: "exec:go test"}
	e.Check(ctx, exec)
	if o := e.Check(ctx, Request{Risk: RiskExec, Command: "go test ./b", Key: "exec:go test"}); o.How != "always" || ap.calls != 1 {
		t.Fatalf("always not remembered: %+v calls=%d", o, ap.calls)
	}
	if o := e.Check(ctx, Request{Risk: RiskExec, Command: "make lint"}); o.How != "auto" {
		t.Fatalf("allow_commands: %+v", o)
	}
	if o := e.Check(ctx, Request{Risk: RiskExec, Command: "make deploy-prod"}); o.Allowed || o.How != "policy" {
		t.Fatalf("deny_commands: %+v", o)
	}
	if o := e.Check(ctx, Request{Risk: RiskWrite, Path: "secrets/x.yaml"}); o.Allowed {
		t.Fatal("deny_paths ignored")
	}
}

func TestHeadlessFailsClosed(t *testing.T) {
	e := NewEngine(Policy{Mode: "default"}, Deny{})
	if o := e.Check(context.Background(), Request{Risk: RiskWrite}); o.Allowed || o.Feedback == "" {
		t.Fatalf("headless: %+v", o)
	}
}
