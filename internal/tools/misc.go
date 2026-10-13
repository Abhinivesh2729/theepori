package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

// --------------------------------------------------------------- git_status

type GitStatus struct{}

func (GitStatus) Name() string        { return "git_status" }
func (GitStatus) Description() string { return "Show git branch, working-tree status and a diffstat." }
func (GitStatus) Schema() string {
	return `{"type":"object","properties":{},"additionalProperties":false}`
}

func (t GitStatus) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	if err := decode(input, &struct{}{}); err != nil {
		return nil, err
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "git status"},
		Run: func(ctx context.Context) Result {
			if !env.WS.IsGit {
				return errResult("not a git repository")
			}
			st := env.WS.Status()
			stat, _ := workspace.Git(env.WS.Root, "diff", "--stat", "HEAD")
			return Result{Output: st + "\n\n" + stat, Display: env.WS.Branch()}
		},
	}, nil
}

// ----------------------------------------------------------------- git_diff

type GitDiff struct{}

func (GitDiff) Name() string { return "git_diff" }
func (GitDiff) Description() string {
	return "Show git diff of the working tree (or staged changes, or against a base ref)."
}
func (GitDiff) Schema() string {
	return `{"type":"object","properties":{"staged":{"type":"boolean"},"base":{"type":"string","description":"Compare against this ref, e.g. main"},"path":{"type":"string"}},"additionalProperties":false}`
}

func (t GitDiff) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Staged bool   `json:"staged"`
		Base   string `json:"base"`
		Path   string `json:"path"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if strings.HasPrefix(in.Base, "-") {
		return nil, fmt.Errorf("invalid base ref")
	}
	args := []string{"diff"}
	switch {
	case in.Staged:
		args = append(args, "--cached")
	case in.Base != "":
		args = append(args, in.Base+"...")
	}
	if in.Path != "" {
		_, rel, err := env.WS.Resolve(in.Path)
		if err != nil {
			return nil, err
		}
		args = append(args, "--", rel)
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "git " + strings.Join(args, " ")},
		Run: func(ctx context.Context) Result {
			if !env.WS.IsGit {
				return errResult("not a git repository")
			}
			out, err := workspace.Git(env.WS.Root, args...)
			if err != nil {
				return errResult("%v", err)
			}
			if out == "" {
				return Result{Output: "No differences.", Display: "no changes"}
			}
			add, del := DiffStat(out)
			return Result{Output: env.Redact.Redact(clip(out, maxOutput)), Display: fmt.Sprintf("+%d -%d", add, del)}
		},
	}, nil
}

// -------------------------------------------------------------- update_plan

type PlanStep struct {
	Step   string `json:"step"`
	Status string `json:"status"` // pending | in_progress | completed
}

type UpdatePlan struct{}

func (UpdatePlan) Name() string { return "update_plan" }
func (UpdatePlan) Description() string {
	return "Publish or update the implementation plan as a checklist the user can see. Call it before starting non-trivial work and whenever a step changes status. Keep exactly one step in_progress."
}
func (UpdatePlan) Schema() string {
	return `{"type":"object","properties":{"explanation":{"type":"string"},"steps":{"type":"array","items":{"type":"object","properties":{"step":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed"]}},"required":["step","status"],"additionalProperties":false}}},"required":["steps"],"additionalProperties":false}`
}

func (t UpdatePlan) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Explanation string     `json:"explanation"`
		Steps       []PlanStep `json:"steps"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if len(in.Steps) == 0 {
		return nil, fmt.Errorf("steps must not be empty")
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskNone, Summary: "update plan"},
		Run: func(ctx context.Context) Result {
			if env.OnPlan != nil {
				env.OnPlan(in.Steps, in.Explanation)
			}
			done := 0
			for _, s := range in.Steps {
				if s.Status == "completed" {
					done++
				}
			}
			return Result{Output: "Plan updated.", Display: fmt.Sprintf("%d/%d done", done, len(in.Steps))}
		},
	}, nil
}

// ----------------------------------------------------------------- ask_user

type AskUser struct{}

func (AskUser) Name() string { return "ask_user" }
func (AskUser) Description() string {
	return "Ask the user a clarifying question when requirements are ambiguous and a wrong guess would be costly. Do not use for routine confirmations."
}
func (AskUser) Schema() string {
	return `{"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false}`
}

func (t AskUser) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Question string `json:"question"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskNone, Summary: "ask: " + in.Question},
		Run: func(ctx context.Context) Result {
			if env.Ask == nil {
				return Result{Output: "No user is available (non-interactive run). Proceed with the most reasonable assumption and state it in your final answer.", Display: "no user available"}
			}
			ans, err := env.Ask(ctx, in.Question)
			if err != nil {
				return errResult("%v", err)
			}
			if strings.TrimSpace(ans) == "" {
				ans = "(no answer — use your best judgement and state the assumption)"
			}
			return Result{Output: "User answered: " + ans, Display: ans}
		},
	}, nil
}
