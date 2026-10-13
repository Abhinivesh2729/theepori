// Package permission decides whether a tool action may run: by policy mode,
// command allow/deny lists, per-session approvals and, when needed, by asking
// the user. Every decision is written to the audit log.
package permission

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"github.com/bmatcuk/doublestar/v4"
)

type Risk int

const (
	RiskNone        Risk = iota // bookkeeping (plans, questions)
	RiskRead                    // reads files, read-only commands
	RiskWrite                   // modifies files in the project
	RiskExec                    // runs arbitrary commands
	RiskDestructive             // irreversible / outward-facing
)

func (r Risk) String() string {
	return [...]string{"none", "read", "write", "exec", "destructive"}[r]
}

// Request describes a proposed action.
type Request struct {
	Tool    string
	Risk    Risk
	Summary string // one line: what will happen
	Detail  string // diff or full command, shown before approval
	Reason  string // why it is risky (destructive matches)
	Key     string // approval memory key, e.g. "exec:go test"
	Command string // for allow/deny command matching
	Path    string // repo-relative path for deny_paths
}

type Answer struct {
	Allow    bool
	Always   bool   // remember for this session
	Feedback string // user guidance returned to the model on denial
}

// Approver asks the human. Interactive UIs implement it; headless runs use
// Deny (fail closed).
type Approver interface {
	Approve(ctx context.Context, req Request) (Answer, error)
}

type Policy struct {
	Mode          string
	AllowCommands []string
	DenyCommands  []string
	DenyPaths     []string
}

type Outcome struct {
	Allowed  bool
	How      string // auto | approved | always | denied | policy
	Feedback string
}

type Engine struct {
	mu       sync.Mutex
	Policy   Policy
	Approver Approver
	always   map[string]bool
}

func NewEngine(p Policy, a Approver) *Engine {
	return &Engine{Policy: p, Approver: a, always: map[string]bool{}}
}

func (e *Engine) SetMode(mode string) {
	e.mu.Lock()
	e.Policy.Mode = mode
	e.mu.Unlock()
}

func (e *Engine) Mode() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.Policy.Mode
}

// Check decides on req, asking the approver when policy says "ask".
func (e *Engine) Check(ctx context.Context, req Request) Outcome {
	e.mu.Lock()
	pol := e.Policy
	remembered := req.Key != "" && e.always[req.Key]
	e.mu.Unlock()

	if req.Path != "" {
		for _, pat := range pol.DenyPaths {
			if ok, _ := doublestar.Match(pat, req.Path); ok {
				return Outcome{How: "policy", Feedback: fmt.Sprintf("path %s is denied by permissions.deny_paths (%s)", req.Path, pat)}
			}
		}
	}
	if req.Command != "" {
		for _, pat := range pol.DenyCommands {
			if matchCommand(pat, req.Command) {
				return Outcome{How: "policy", Feedback: fmt.Sprintf("command denied by permissions.deny_commands (%s)", pat)}
			}
		}
	}

	decision := decide(pol.Mode, req.Risk)
	if decision == "ask" && req.Risk == RiskExec && req.Command != "" {
		for _, pat := range pol.AllowCommands {
			if matchCommand(pat, req.Command) {
				decision = "allow"
			}
		}
	}
	if decision == "ask" && remembered && req.Risk < RiskDestructive {
		return Outcome{Allowed: true, How: "always"}
	}
	switch decision {
	case "allow":
		return Outcome{Allowed: true, How: "auto"}
	case "deny":
		return Outcome{How: "policy", Feedback: fmt.Sprintf("%s actions are not allowed in %q mode", req.Risk, pol.Mode)}
	}
	if e.Approver == nil {
		return Outcome{How: "denied", Feedback: "approval required but no user is available (non-interactive run); rerun interactively or use --mode"}
	}
	ans, err := e.Approver.Approve(ctx, req)
	if err != nil {
		return Outcome{How: "denied", Feedback: "approval failed: " + err.Error()}
	}
	if !ans.Allow {
		return Outcome{How: "denied", Feedback: ans.Feedback}
	}
	if ans.Always && req.Key != "" && req.Risk < RiskDestructive {
		e.mu.Lock()
		e.always[req.Key] = true
		e.mu.Unlock()
		return Outcome{Allowed: true, How: "always"}
	}
	return Outcome{Allowed: true, How: "approved"}
}

// decide returns allow | ask | deny for a mode and risk.
func decide(mode string, r Risk) string {
	if r <= RiskRead {
		return "allow"
	}
	if r == RiskDestructive {
		if mode == "plan" {
			return "deny"
		}
		return "ask" // always explicit, in every mode
	}
	switch mode {
	case "plan":
		return "deny"
	case "accept-edits":
		if r == RiskWrite {
			return "allow"
		}
	case "auto":
		return "allow"
	}
	return "ask"
}

// matchCommand matches a glob ("go test *") or exact prefix ("go test").
func matchCommand(pat, cmd string) bool {
	cmd = strings.TrimSpace(cmd)
	pat = strings.TrimSpace(pat)
	if strings.ContainsAny(pat, "*?[") {
		ok, _ := doublestar.Match(pat, cmd)
		return ok
	}
	return cmd == pat || strings.HasPrefix(cmd, pat+" ")
}

// Deny is the headless approver: every "ask" fails closed.
type Deny struct{}

func (Deny) Approve(context.Context, Request) (Answer, error) {
	return Answer{Feedback: "approval required but tp is running non-interactively; rerun in a terminal or relax --mode"}, nil
}
