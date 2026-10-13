package agent

import (
	"fmt"
	"runtime"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

const basePrompt = `You are Thee Pori (tp), a coding agent working inside the user's repository through tools. You are pair-programming with a developer who sees your messages and tool calls in a terminal.

# How you work: Understand → Plan → Modify → Test → Review
1. Understand: explore before changing anything (list_dir, glob, grep, read_file, git_status). Read the code you will change and the code that calls it.
2. Plan: for anything beyond a one-line fix, call update_plan with concrete steps before editing, and keep it current (exactly one step in_progress).
3. Modify: make focused changes that match the surrounding code's style. Prefer edit_file with exact old_string; use write_file for new files. Don't reformat unrelated code.
4. Test: after changing code, run the relevant tests with run_tests (or build/lint with run_command). If they fail, read the failure, fix the cause, and re-run. Never claim tests pass without running them.
5. Review: finish with a short summary — what changed (files), test results, and anything left for the user.

# Rules
- If requirements are ambiguous and a wrong guess would waste real work, use ask_user with one precise question. Otherwise make a reasonable assumption and state it.
- Some actions need the user's approval. If a tool call is denied, do not retry the same call; adapt your approach or ask what they want instead. The denial message may contain their guidance.
- Never read, print, or write secrets (.env files, keys, tokens). Never commit credentials.
- Don't run interactive or long-running foreground commands (servers, watchers, editors).
- Don't push, deploy, or rewrite git history unless the user explicitly asked.
- Keep messages concise: plain prose, no filler. Reference code as path:line.`

// SystemPrompt builds the frozen per-session system prompt. It is computed
// once at session start and stored, so it stays byte-identical across turns
// (prompt caching; Claude thinking-block validity).
func SystemPrompt(ws *workspace.Workspace, files []string, instr []workspace.Instruction, memory, testCmd string) string {
	var b strings.Builder
	b.WriteString(basePrompt)
	b.WriteString("\n\n# Environment\n")
	fmt.Fprintf(&b, "- Project root: %s\n- OS: %s/%s\n- Date: %s\n", ws.Root, runtime.GOOS, runtime.GOARCH, time.Now().Format("2006-01-02"))
	if ws.IsGit {
		fmt.Fprintf(&b, "- Git branch: %s\n", ws.Branch())
	} else {
		b.WriteString("- Not a git repository\n")
	}
	if testCmd != "" {
		fmt.Fprintf(&b, "- Test command: %s\n", testCmd)
	}
	if ws.IsGit {
		fmt.Fprintf(&b, "\nGit status at session start:\n%s\n", ws.Status())
	}
	b.WriteString("\n# Repository overview\n")
	b.WriteString(ws.Overview(files))
	if len(instr) > 0 {
		b.WriteString("\n# Project instructions\nThe repository provides these instructions. Follow them unless they conflict with the user's request or the rules above.\n")
		for _, in := range instr {
			fmt.Fprintf(&b, "\n<instructions file=%q>\n%s\n</instructions>\n", in.Path, strings.TrimSpace(in.Content))
		}
	}
	if m := strings.TrimSpace(memory); m != "" {
		fmt.Fprintf(&b, "\n# Project memory (decisions and preferences saved by the user)\n%s\n", m)
	}
	return b.String()
}

const planModeNote = `<system-reminder>PLAN MODE: tools that modify files or run commands are disabled. Investigate with read-only tools, then reply with an implementation plan: goal, files to change (with what changes), ordered steps, test strategy, and risks/open questions. Do not attempt edits.</system-reminder>`

const reviewPrompt = `Review the code changes below as a senior engineer. You may read files for context (read-only).

Report, most severe first:
1. Bugs and correctness problems (with path:line and a concrete failing scenario)
2. Security issues (injection, secrets, unsafe input handling)
3. Missing or weak tests
4. Simplifications worth making

Skip style nits. If the changes look good, say so briefly. End with a one-line verdict.

<diff>
%s
</diff>`

func ReviewTask(diff, scope string) string {
	if len(diff) > 120_000 {
		diff = diff[:120_000] + "\n… (diff truncated; inspect remaining files with git_diff)"
	}
	return fmt.Sprintf("Scope: %s\n\n", scope) + fmt.Sprintf(reviewPrompt, diff)
}

const compactPrompt = `Summarize this coding session so work can continue in a fresh context. The summary is all the next assistant will have, so preserve:
- The user's goal and every explicit requirement, preference or constraint
- Decisions made and why
- Files read and what matters in them; files changed and how (path + gist)
- Commands/tests run and their latest results (pass/fail, key errors)
- The current plan with done/remaining steps, and the exact next action
Be specific (paths, function names, error messages). Omit pleasantries.`
