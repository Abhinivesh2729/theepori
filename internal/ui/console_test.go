package ui

import (
	"bytes"
	"context"
	"strings"
	"testing"

	"github.com/Abhinivesh2729/theepori/internal/permission"
)

func approve(t *testing.T, input string, risk permission.Risk) (permission.Answer, string) {
	t.Helper()
	var out bytes.Buffer
	c := &Console{Out: &out, Interactive: true, interrupt: make(chan struct{}, 1), atBOL: true}
	c.StartInput(strings.NewReader(input))
	ans, err := c.Approve(context.Background(), permission.Request{Tool: "edit_file", Risk: risk, Summary: "edit a.go", Detail: "--- a/a.go\n+++ b/a.go\n@@ -1 +1 @@\n-x\n+y\n"})
	if err != nil {
		t.Fatal(err)
	}
	return ans, out.String()
}

func TestApproveRepromptsOnSlashCommand(t *testing.T) {
	ans, out := approve(t, "/status\ny\n", permission.RiskWrite)
	if !ans.Allow || !strings.Contains(out, "answer first") {
		t.Fatalf("ans=%+v out=%q", ans, out)
	}
	ans, _ = approve(t, "/tmp is wrong, use ./build\n", permission.RiskWrite)
	if ans.Allow || ans.Feedback != "/tmp is wrong, use ./build" {
		t.Fatalf("path-like feedback must pass through: %+v", ans)
	}
	ans, _ = approve(t, "/diff\nyes\n", permission.RiskDestructive)
	if !ans.Allow {
		t.Fatalf("destructive path: %+v", ans)
	}
	ans, _ = approve(t, "y\n", permission.RiskDestructive)
	if ans.Allow {
		t.Fatal("destructive needs a typed yes")
	}
}

func TestTamilClusters(t *testing.T) {
	got := strings.Join(Clusters("தீப்பொறி"), "|")
	if got != "தீ|ப்|பொ|றி" {
		t.Fatalf("clusters = %s", got)
	}
}

func TestLogoPlainWithoutColor(t *testing.T) {
	c := &Console{Color: false}
	logo := c.Logo("v1")
	if strings.Contains(logo, "\x1b[") || !strings.Contains(logo, "தீப்பொறி") || strings.Count(logo, "\n") != len(flame) {
		t.Fatalf("logo:\n%s", logo)
	}
}
