package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

// --------------------------------------------------------------- run_command

type RunCommand struct{}

func (RunCommand) Name() string { return "run_command" }
func (RunCommand) Description() string {
	return "Run a shell command in the project root (sh -c). Returns combined stdout/stderr, exit code and duration. " +
		"Use for builds, linters, package managers and git. Use run_tests for the test suite. Avoid interactive commands."
}
func (RunCommand) Schema() string {
	return `{"type":"object","properties":{"command":{"type":"string"},"timeout_sec":{"type":"integer","description":"Default 120, max 1800"},"workdir":{"type":"string","description":"Subdirectory to run in (default project root)"}},"required":["command"],"additionalProperties":false}`
}

func (t RunCommand) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Command    string `json:"command"`
		TimeoutSec int    `json:"timeout_sec"`
		Workdir    string `json:"workdir"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if strings.TrimSpace(in.Command) == "" {
		return nil, fmt.Errorf("command is required")
	}
	dir := env.WS.Root
	if in.Workdir != "" {
		abs, _, err := env.WS.Resolve(in.Workdir)
		if err != nil {
			return nil, err
		}
		dir = abs
	}
	risk, reason := permission.ClassifyCommand(in.Command)
	if commitRe.MatchString(in.Command) {
		if err := checkStagedSecrets(env); err != nil {
			return nil, err
		}
	}
	return &Call{
		Perm: permission.Request{
			Tool: t.Name(), Risk: risk, Reason: reason, Command: in.Command,
			Key: permission.CommandKey(in.Command), Summary: "$ " + in.Command, Detail: in.Command,
		},
		Run: func(ctx context.Context) Result {
			res := runShell(ctx, env, dir, in.Command, timeout(in.TimeoutSec))
			env.Tracker.AddCommand(CommandRecord{Command: in.Command, ExitCode: *res.ExitCode, Duration: res.Duration, At: time.Now()})
			return res
		},
	}, nil
}

func timeout(sec int) time.Duration {
	switch {
	case sec <= 0:
		return 120 * time.Second
	case sec > 1800:
		return 1800 * time.Second
	}
	return time.Duration(sec) * time.Second
}

// runShell executes cmd with a process-group kill on timeout/cancel and API
// keys removed from the child environment.
func runShell(ctx context.Context, env *Env, dir, command string, limit time.Duration) Result {
	ctx, cancel := context.WithTimeout(ctx, limit)
	defer cancel()
	cmd := exec.Command("/bin/sh", "-c", command)
	cmd.Dir = dir
	cmd.Env = childEnv(env.StripEnv)
	cmd.Stdin = nil
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	var buf syncBuffer
	cmd.Stdout, cmd.Stderr = &buf, &buf
	start := time.Now()
	if err := cmd.Start(); err != nil {
		code := -1
		return Result{Output: "Error: " + err.Error(), IsError: true, ExitCode: &code, Display: err.Error()}
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	var err error
	timedOut := false
	select {
	case err = <-done:
	case <-ctx.Done():
		_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)
		select {
		case err = <-done:
		case <-time.After(3 * time.Second):
			_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
			err = <-done
		}
		timedOut = errors.Is(ctx.Err(), context.DeadlineExceeded)
	}
	dur := time.Since(start)
	code := 0
	if err != nil {
		code = -1
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			code = ee.ExitCode()
		}
	}
	out := env.Redact.Redact(clip(buf.String(), maxOutput))
	var b strings.Builder
	b.WriteString(out)
	if out != "" && !strings.HasSuffix(out, "\n") {
		b.WriteString("\n")
	}
	switch {
	case timedOut:
		fmt.Fprintf(&b, "[timed out after %s]\n", limit)
	case ctx.Err() != nil:
		b.WriteString("[canceled]\n")
	}
	fmt.Fprintf(&b, "[exit code %d, %s]", code, dur.Round(time.Millisecond))
	return Result{
		Output: b.String(), IsError: code != 0, ExitCode: &code, Duration: dur,
		Display: fmt.Sprintf("exit %d · %s", code, dur.Round(100*time.Millisecond)),
	}
}

func childEnv(strip []string) []string {
	drop := map[string]bool{}
	for _, k := range strip {
		drop[k] = true
	}
	var out []string
	for _, kv := range os.Environ() {
		k, _, _ := strings.Cut(kv, "=")
		if !drop[k] {
			out = append(out, kv)
		}
	}
	return append(out, "TP_AGENT=1", "GIT_TERMINAL_PROMPT=0", "CI=1")
}

type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.b.Len() > 8<<20 { // cap memory; clip keeps head/tail anyway
		return len(p), nil
	}
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

var commitRe = regexp.MustCompile(`\bgit\s+commit\b`)

// checkStagedSecrets blocks commits that include secret files or values.
func checkStagedSecrets(env *Env) error {
	if !env.WS.IsGit {
		return nil
	}
	names, _ := workspace.Git(env.WS.Root, "diff", "--cached", "--name-only")
	for _, n := range strings.Split(strings.TrimSpace(names), "\n") {
		if n != "" && workspace.Sensitive(n, nil) {
			return fmt.Errorf("refusing to commit: staged file %s looks like a secret. Unstage it (git restore --staged %s) and add it to .gitignore", n, n)
		}
	}
	diff, _ := workspace.Git(env.WS.Root, "diff", "--cached", "-U0")
	var added strings.Builder
	for _, l := range strings.Split(diff, "\n") {
		if strings.HasPrefix(l, "+") && !strings.HasPrefix(l, "+++") {
			added.WriteString(l + "\n")
		}
	}
	if env.Redact.Contains(added.String()) {
		return fmt.Errorf("refusing to commit: staged changes appear to contain a credential. Move it to an env var or secrets manager")
	}
	return nil
}

// ---------------------------------------------------------------- run_tests

type RunTests struct{}

func (RunTests) Name() string { return "run_tests" }
func (RunTests) Description() string {
	return "Run the project's test suite (configured test_command or auto-detected) or a specific test command. Returns output, exit code and a pass/fail summary."
}
func (RunTests) Schema() string {
	return `{"type":"object","properties":{"command":{"type":"string","description":"Override, e.g. 'go test ./pkg/...' or 'pytest tests/test_x.py'"},"timeout_sec":{"type":"integer","description":"Default 600"}},"additionalProperties":false}`
}

func (t RunTests) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Command    string `json:"command"`
		TimeoutSec int    `json:"timeout_sec"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	cmd := in.Command
	if cmd == "" {
		cmd = env.TestCommand
	}
	if cmd == "" {
		cmd = workspace.DetectTestCommand(env.WS.Root)
	}
	if cmd == "" {
		return nil, fmt.Errorf("no test command configured or detected; pass command, or set test_command in .theepori/config.yaml")
	}
	if in.TimeoutSec <= 0 {
		in.TimeoutSec = 600
	}
	risk, reason := permission.ClassifyCommand(cmd)
	if risk < permission.RiskExec {
		risk = permission.RiskExec
	}
	return &Call{
		Perm: permission.Request{
			Tool: t.Name(), Risk: risk, Reason: reason, Command: cmd,
			Key: permission.CommandKey(cmd), Summary: "test: $ " + cmd, Detail: cmd,
		},
		Run: func(ctx context.Context) Result {
			res := runShell(ctx, env, env.WS.Root, cmd, timeout(in.TimeoutSec))
			passed := res.ExitCode != nil && *res.ExitCode == 0
			summary := SummarizeTests(res.Output, passed)
			env.Tracker.AddTest(TestRecord{Command: cmd, ExitCode: *res.ExitCode, Passed: passed, Summary: summary, Duration: res.Duration, At: time.Now()})
			status := "PASSED"
			if !passed {
				status = "FAILED"
			}
			res.Output = fmt.Sprintf("Tests %s: %s\n\n%s", status, summary, res.Output)
			res.Display = fmt.Sprintf("%s · %s · %s", strings.ToLower(status), summary, res.Duration.Round(100*time.Millisecond))
			return res
		},
	}, nil
}

var (
	pytestRe     = regexp.MustCompile(`(\d+ (passed|failed|error|errors|skipped)(, )?)+`)
	jestRe       = regexp.MustCompile(`Tests:\s+[^\n]+`)
	cargoRe      = regexp.MustCompile(`test result: [^\n]+`)
	goOkRe       = regexp.MustCompile(`(?m)^ok\s`)
	goFailPkgRe  = regexp.MustCompile(`(?m)^FAIL[ \t]+\S+`)
	goFailTestRe = regexp.MustCompile(`(?m)^--- FAIL:`) // top-level tests only
)

// SummarizeTests extracts a one-line summary from common runners' output.
func SummarizeTests(out string, passed bool) string {
	if m := jestRe.FindString(out); m != "" {
		return strings.TrimSpace(m)
	}
	if ms := cargoRe.FindAllString(out, -1); len(ms) > 0 {
		return ms[len(ms)-1]
	}
	if ms := pytestRe.FindAllString(out, -1); len(ms) > 0 {
		return ms[len(ms)-1]
	}
	ok, failPkg, failTest := len(goOkRe.FindAllString(out, -1)), len(goFailPkgRe.FindAllString(out, -1)), len(goFailTestRe.FindAllString(out, -1))
	switch {
	case failTest > 0:
		return fmt.Sprintf("%d test(s) failed in %d package(s), %d package(s) ok", failTest, failPkg, ok)
	case failPkg > 0:
		return fmt.Sprintf("%d package(s) failed to build or panicked, %d package(s) ok", failPkg, ok)
	case ok > 0:
		return fmt.Sprintf("%d package(s) ok", ok)
	}
	if passed {
		return "exit 0"
	}
	return "non-zero exit"
}
