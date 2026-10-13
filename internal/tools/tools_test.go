package tools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/secrets"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

func testEnv(t *testing.T) *Env {
	t.Helper()
	root := t.TempDir()
	root, _ = filepath.EvalSymlinks(root)
	ws := workspace.Detect(root, nil)
	return &Env{WS: ws, Redact: secrets.New("TP_TEST_KEY"), Tracker: NewTracker(ws.Root, nil), MaxFileBytes: 1 << 20, StripEnv: []string{"TP_TEST_KEY"}}
}

func run(t *testing.T, env *Env, tool Tool, input string) (Result, *Call, error) {
	t.Helper()
	call, err := tool.Prepare(env, json.RawMessage(input))
	if err != nil {
		return Result{}, nil, err
	}
	return call.Run(context.Background()), call, nil
}

func write(t *testing.T, env *Env, rel, content string) {
	t.Helper()
	p := filepath.Join(env.WS.Root, rel)
	os.MkdirAll(filepath.Dir(p), 0o755)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestEditFile(t *testing.T) {
	env := testEnv(t)
	write(t, env, "a.go", "package a\n\nfunc A() int { return 1 }\nfunc B() int { return 1 }\n")
	if _, _, err := run(t, env, EditFile{}, `{"path":"a.go","old_string":"return 1","new_string":"return 2"}`); err == nil || !strings.Contains(err.Error(), "2 times") {
		t.Fatalf("ambiguous edit: %v", err)
	}
	if _, _, err := run(t, env, EditFile{}, `{"path":"a.go","old_string":"func  A()","new_string":"x"}`); err == nil || !strings.Contains(err.Error(), "whitespace") {
		t.Fatalf("whitespace hint: %v", err)
	}
	env.Tracker.StartTurn()
	res, call, err := run(t, env, EditFile{}, `{"path":"a.go","old_string":"func A() int { return 1 }","new_string":"func A() int { return 42 }"}`)
	if err != nil || res.IsError {
		t.Fatalf("edit: %v %v", err, res.Output)
	}
	if call.Perm.Risk != permission.RiskWrite || !strings.Contains(call.Perm.Detail, "+func A() int { return 42 }") {
		t.Fatalf("preview: %+v", call.Perm)
	}
	b, _ := os.ReadFile(filepath.Join(env.WS.Root, "a.go"))
	if !strings.Contains(string(b), "return 42") {
		t.Fatal("file not edited")
	}
	if ch := env.Tracker.Changed(); len(ch) != 1 || ch[0] != "a.go" {
		t.Fatalf("changed = %v", ch)
	}
	if files, err := env.Tracker.Undo(); err != nil || len(files) != 1 {
		t.Fatalf("undo: %v %v", files, err)
	}
	b, _ = os.ReadFile(filepath.Join(env.WS.Root, "a.go"))
	if strings.Contains(string(b), "return 42") {
		t.Fatal("undo did not restore")
	}
}

func TestWriteFileCreateAndUndoDeletes(t *testing.T) {
	env := testEnv(t)
	env.Tracker.StartTurn()
	res, call, err := run(t, env, WriteFile{}, `{"path":"pkg/new.txt","content":"hello\n"}`)
	if err != nil || res.IsError || !strings.HasPrefix(call.Perm.Summary, "create") {
		t.Fatalf("%v %v %+v", err, res, call)
	}
	if _, err := env.Tracker.Undo(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(env.WS.Root, "pkg/new.txt")); !os.IsNotExist(err) {
		t.Fatal("undo should delete a created file")
	}
}

func TestTruncatedInputRejected(t *testing.T) {
	env := testEnv(t)
	if _, _, err := run(t, env, WriteFile{}, `{"path":"a.txt","content":"unterminated`); err == nil {
		t.Fatal("truncated JSON must fail")
	}
	if _, _, err := run(t, env, WriteFile{}, `{"path":"a.txt"}`); err == nil {
		t.Fatal("missing content must fail")
	}
}

func TestSandboxAndSecrets(t *testing.T) {
	env := testEnv(t)
	write(t, env, ".env", "TOKEN=abc")
	write(t, env, "config.yaml", "api_key: sk-ant-REALLYSECRETVALUE1234567890\n")
	if _, _, err := run(t, env, ReadFile{}, `{"path":"../outside.txt"}`); err == nil {
		t.Fatal("path escape allowed")
	}
	if _, _, err := run(t, env, ReadFile{}, `{"path":".env"}`); err == nil {
		t.Fatal(".env must not be readable")
	}
	if _, _, err := run(t, env, WriteFile{}, `{"path":"keys/server.pem","content":"x"}`); err == nil {
		t.Fatal(".pem must not be writable")
	}
	res, _, err := run(t, env, ReadFile{}, `{"path":"config.yaml"}`)
	if err != nil || strings.Contains(res.Output, "REALLYSECRET") {
		t.Fatalf("secret not redacted: %v %q", err, res.Output)
	}
	res, _, _ = run(t, env, Glob{}, `{"pattern":"*"}`)
	if strings.Contains(res.Output, ".env") {
		t.Fatal("glob must hide sensitive files")
	}
	if workspace.Sensitive(".env.example", nil) {
		t.Fatal(".env.example is safe")
	}
}

func TestRunCommand(t *testing.T) {
	env := testEnv(t)
	t.Setenv("TP_TEST_KEY", "supersecretvalue123")
	res, call, err := run(t, env, RunCommand{}, `{"command":"echo out; echo key=$TP_TEST_KEY; exit 3"}`)
	if err != nil {
		t.Fatal(err)
	}
	if call.Perm.Risk != permission.RiskExec || *res.ExitCode != 3 || !res.IsError {
		t.Fatalf("res = %+v", res)
	}
	if strings.Contains(res.Output, "supersecret") || !strings.Contains(res.Output, "out") {
		t.Fatalf("api key leaked or output missing: %q", res.Output)
	}
	res, _, _ = run(t, env, RunCommand{}, `{"command":"sleep 5","timeout_sec":1}`)
	if !strings.Contains(res.Output, "timed out") {
		t.Fatalf("timeout: %q", res.Output)
	}
}

func TestRunTestsSummary(t *testing.T) {
	env := testEnv(t)
	env.TestCommand = "printf 'ok  \tpkg/a\t0.1s\n--- FAIL: TestX\nFAIL\tpkg/b\n'; exit 1"
	res, _, err := run(t, env, RunTests{}, `{}`)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(res.Output, "Tests FAILED") {
		t.Fatalf("output: %q", res.Output)
	}
	tests := env.Tracker.State().Tests
	if len(tests) != 1 || tests[0].Passed || tests[0].Summary != "1 test(s) failed in 1 package(s), 1 package(s) ok" {
		t.Fatalf("tests = %+v", tests)
	}
	if got := SummarizeTests("==== 3 passed, 1 failed in 0.2s ====", false); got != "3 passed, 1 failed" {
		t.Fatalf("pytest summary = %q", got)
	}
}

func TestGrepAndList(t *testing.T) {
	env := testEnv(t)
	write(t, env, "src/app.go", "package src\n// TODO fix\n")
	write(t, env, "src/util.go", "package src\n")
	res, _, err := run(t, env, Grep{}, `{"pattern":"todo","ignore_case":true}`)
	if err != nil || !strings.Contains(res.Output, "src/app.go:2:") {
		t.Fatalf("grep: %v %q", err, res.Output)
	}
	res, _, _ = run(t, env, ListDir{}, `{}`)
	if !strings.Contains(res.Output, "src/app.go") {
		t.Fatalf("list: %q", res.Output)
	}
}

func TestLineStatIgnoresHeaderLookalikes(t *testing.T) {
	old := "select 1;\n-- TODO remove\n-- a/b\nkeep\n"
	new := "select 1;\nkeep\n++ added\n"
	if add, del := LineStat(old, new); add != 1 || del != 2 {
		t.Fatalf("add=%d del=%d", add, del)
	}
	if add, del := LineStat("", "a\nb\n"); add != 2 || del != 0 {
		t.Fatalf("new file: add=%d del=%d", add, del)
	}
}

func TestGoTestSummaryRealOutput(t *testing.T) {
	out := "--- FAIL: TestX (0.00s)\n    --- FAIL: TestX/sub (0.00s)\n        x_test.go:9: boom\nFAIL\nFAIL\texample.com/a\t0.2s\nok  \texample.com/b\t0.1s\nFAIL\nexit status 1\nmake: *** [test] Error 1\n"
	if got := SummarizeTests(out, false); got != "1 test(s) failed in 1 package(s), 1 package(s) ok" {
		t.Fatalf("got %q", got)
	}
	build := "# example.com/c\n./c.go:3:2: undefined: y\nFAIL\texample.com/c [build failed]\nFAIL\n"
	if got := SummarizeTests(build, false); got != "1 package(s) failed to build or panicked, 0 package(s) ok" {
		t.Fatalf("build failure: %q", got)
	}
}
