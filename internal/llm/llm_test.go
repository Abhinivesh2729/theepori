package llm

import (
	"context"
	"strings"
	"testing"
)

func TestParseTextToolCalls(t *testing.T) {
	text := "Let me look.\n<tool_call>\n{\"name\": \"read_file\", \"input\": {\"path\": \"a.go\"}}\n</tool_call>\nand\n<tool_call>{\"name\":\"grep\",\"arguments\":{\"pattern\":\"x\"}}</tool_call>"
	rest, calls := ParseTextToolCalls(text)
	if len(calls) != 2 || calls[0].Name != "read_file" || calls[1].Name != "grep" {
		t.Fatalf("calls = %+v", calls)
	}
	if string(calls[1].Input) != `{"pattern":"x"}` {
		t.Fatalf("arguments alias not parsed: %s", calls[1].Input)
	}
	if strings.Contains(rest, "tool_call") || !strings.Contains(rest, "Let me look.") {
		t.Fatalf("rest = %q", rest)
	}
}

func TestParseTextToolCallsUnclosed(t *testing.T) {
	_, calls := ParseTextToolCalls("ok <tool_call>{\"name\":\"list_dir\",\"input\":{}}")
	if len(calls) != 1 || calls[0].Name != "list_dir" {
		t.Fatalf("calls = %+v", calls)
	}
}

func TestParseTextToolCallsMalformedLeftVisible(t *testing.T) {
	rest, calls := ParseTextToolCalls("<tool_call>{not json}</tool_call>")
	if len(calls) != 0 || !strings.Contains(rest, "not json") {
		t.Fatalf("calls=%v rest=%q", calls, rest)
	}
}

func TestTagFilterAcrossChunks(t *testing.T) {
	f := &tagFilter{open: "<tool_call>", close: "</tool_call>"}
	var out strings.Builder
	for _, chunk := range []string{"Hello <to", "ol_call>{\"name\":", "\"x\"}</tool", "_call> bye"} {
		out.WriteString(f.Feed(chunk))
	}
	if out.String() != "Hello  bye" {
		t.Fatalf("got %q", out.String())
	}
}

type scripted struct {
	reqs []Request
	resp []*Response
	errs []error
}

func (s *scripted) Name() string                                    { return "fake" }
func (s *scripted) ListModels(context.Context) ([]ModelInfo, error) { return nil, nil }
func (s *scripted) Chat(_ context.Context, r Request, on StreamFunc) (*Response, error) {
	s.reqs = append(s.reqs, r)
	i := len(s.reqs) - 1
	if i < len(s.errs) && s.errs[i] != nil {
		return nil, s.errs[i]
	}
	return s.resp[min(i, len(s.resp)-1)], nil
}

func TestClientRetriesThenFallsBack(t *testing.T) {
	rate := &Error{Provider: "a", Kind: ErrRateLimit, RetryAfter: 1}
	primary := &scripted{errs: []error{rate, rate}, resp: []*Response{{}}}
	backup := &scripted{resp: []*Response{{Message: Message{Role: RoleAssistant, Parts: []Part{{Type: PartText, Text: "hi"}}}}}}
	var notes []string
	c := &Client{MaxRetries: 1, Notify: func(s string) { notes = append(notes, s) },
		Targets: []Target{{Ref: "a/x", Provider: primary, Model: ModelInfo{ID: "x"}}, {Ref: "b/y", Provider: backup, Model: ModelInfo{ID: "y"}}}}
	resp, tgt, err := c.Chat(context.Background(), Request{}, nil)
	if err != nil || tgt.Ref != "b/y" || resp.Message.Text() != "hi" {
		t.Fatalf("resp=%v tgt=%v err=%v", resp, tgt.Ref, err)
	}
	if len(primary.reqs) != 2 || backup.reqs[0].Model != "y" {
		t.Fatalf("primary calls=%d backup model=%q", len(primary.reqs), backup.reqs[0].Model)
	}
	if len(notes) < 2 {
		t.Fatalf("expected retry+fallback notices, got %v", notes)
	}
}

func TestClientNoFallbackOnAuth(t *testing.T) {
	primary := &scripted{errs: []error{&Error{Provider: "a", Kind: ErrAuth}}}
	backup := &scripted{resp: []*Response{{}}}
	c := &Client{MaxRetries: 3, Targets: []Target{{Provider: primary}, {Provider: backup}}}
	if _, _, err := c.Chat(context.Background(), Request{}, nil); err == nil {
		t.Fatal("expected auth error")
	}
	if len(backup.reqs) != 0 {
		t.Fatal("auth errors must not fall back")
	}
}

func TestTextToolsWrapper(t *testing.T) {
	inner := &scripted{resp: []*Response{{Message: Message{Role: RoleAssistant, Parts: []Part{{Type: PartText, Text: "checking\n<tool_call>{\"name\":\"glob\",\"input\":{\"pattern\":\"*.go\"}}</tool_call>"}}}, StopReason: StopEnd}}}
	p := TextTools(inner)
	resp, err := p.Chat(context.Background(), Request{
		System: "sys",
		Tools:  []ToolSpec{{Name: "glob", Description: "find", Schema: []byte(`{"type":"object"}`)}},
		Messages: []Message{
			{Role: RoleAssistant, Parts: []Part{{Type: PartToolCall, ToolCall: &ToolCall{ID: "1", Name: "glob", Input: []byte(`{}`)}}}},
			{Role: RoleUser, Parts: []Part{{Type: PartToolResult, ToolResult: &ToolResult{CallID: "1", Name: "glob", Content: "a.go"}}}},
		},
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StopReason != StopToolUse || len(resp.Message.ToolCalls()) != 1 {
		t.Fatalf("resp = %+v", resp)
	}
	sent := inner.reqs[0]
	if len(sent.Tools) != 0 || !strings.Contains(sent.System, "### glob") {
		t.Fatal("tools should move into the system prompt")
	}
	if !strings.Contains(sent.Messages[1].Text(), "<tool_result name=\"glob\"") {
		t.Fatalf("tool results not flattened: %q", sent.Messages[1].Text())
	}
}

func TestFromStatus(t *testing.T) {
	if e := FromStatus("p", 400, `{"error":"prompt is too long"}`, nil); e.Kind != ErrContextLength {
		t.Fatalf("kind = %s", e.Kind)
	}
	if e := FromStatus("p", 401, "", nil); e.Kind != ErrAuth || e.Hint == "" {
		t.Fatalf("auth: %+v", e)
	}
	if e := FromStatus("p", 503, "", nil); !e.Retryable() {
		t.Fatal("5xx must be retryable")
	}
}

func TestCost(t *testing.T) {
	m := ModelInfo{InputPrice: 4, OutputPrice: 20, CacheRead: 0.2}
	got := Cost(m, Usage{InputTokens: 1_000_000, OutputTokens: 100_000, CacheReadTokens: 1_000_000})
	if got < 6.19 || got > 6.21 {
		t.Fatalf("cost = %v", got)
	}
}
