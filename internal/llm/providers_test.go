package llm_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/llm/anthropic"
	"github.com/Abhinivesh2729/theepori/internal/llm/ollama"
	"github.com/Abhinivesh2729/theepori/internal/llm/openai"
)

func TestOpenAIStreamingToolCalls(t *testing.T) {
	var body map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer k" {
			w.WriteHeader(401)
			return
		}
		b, _ := io.ReadAll(r.Body)
		_ = json.Unmarshal(b, &body)
		w.Header().Set("Content-Type", "text/event-stream")
		for _, c := range []string{
			`{"choices":[{"delta":{"content":"Look"}}]}`,
			`{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}`,
			`{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}`,
			`{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":40}}}`,
		} {
			fmt.Fprintf(w, "data: %s\n\n", c)
		}
		fmt.Fprint(w, "data: [DONE]\n\n")
	}))
	defer srv.Close()

	p := openai.New(openai.Config{Name: "oai", Type: "openai", BaseURL: srv.URL, APIKey: "k"})
	var streamed strings.Builder
	resp, err := p.Chat(context.Background(), llm.Request{
		Model: "gpt-5", System: "sys", MaxTokens: 50,
		Messages: []llm.Message{
			llm.UserText("hi"),
			{Role: llm.RoleAssistant, Parts: []llm.Part{{Type: llm.PartToolCall, ToolCall: &llm.ToolCall{ID: "c0", Name: "glob", Input: json.RawMessage(`{}`)}}}},
			{Role: llm.RoleUser, Parts: []llm.Part{{Type: llm.PartToolResult, ToolResult: &llm.ToolResult{CallID: "c0", Name: "glob", Content: "a"}}, {Type: llm.PartText, Text: "more"}}},
		},
		Tools: []llm.ToolSpec{{Name: "read_file", Schema: json.RawMessage(`{"type":"object"}`)}},
	}, func(ev llm.StreamEvent) { streamed.WriteString(ev.Text) })
	if err != nil {
		t.Fatal(err)
	}
	calls := resp.Message.ToolCalls()
	if resp.StopReason != llm.StopToolUse || len(calls) != 1 || string(calls[0].Input) != `{"path":"x"}` || calls[0].ID != "c1" {
		t.Fatalf("resp = %+v", resp)
	}
	if streamed.String() != "Look" || resp.Usage.InputTokens != 60 || resp.Usage.CacheReadTokens != 40 {
		t.Fatalf("streamed=%q usage=%+v", streamed.String(), resp.Usage)
	}
	if body["max_completion_tokens"] == nil {
		t.Fatal("openai type must send max_completion_tokens")
	}
	msgs := body["messages"].([]any)
	roles := []string{}
	for _, m := range msgs {
		roles = append(roles, m.(map[string]any)["role"].(string))
	}
	if strings.Join(roles, ",") != "system,user,assistant,tool,user" {
		t.Fatalf("roles = %v", roles)
	}
}

func TestOpenAIAuthErrorIsActionable(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(401)
		fmt.Fprint(w, `{"error":{"message":"bad key"}}`)
	}))
	defer srv.Close()
	p := openai.New(openai.Config{Name: "oai", BaseURL: srv.URL, APIKey: "x"})
	_, err := p.Chat(context.Background(), llm.Request{Model: "m", Messages: []llm.Message{llm.UserText("hi")}}, nil)
	e, ok := llm.AsError(err)
	if !ok || e.Kind != llm.ErrAuth || !strings.Contains(err.Error(), "tp auth") {
		t.Fatalf("err = %v", err)
	}
}

func TestOllamaThinai(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/":
			fmt.Fprint(w, "Thinai is running. Model: gemma")
		case "/api/ps":
			fmt.Fprint(w, `{"models":[{"name":"gemma-3-1b"}]}`)
		case "/api/chat":
			var body map[string]any
			_ = json.NewDecoder(r.Body).Decode(&body)
			if body["model"] != "gemma-3-1b" {
				t.Errorf("model = %v (should default to the loaded model)", body["model"])
			}
			fmt.Fprintln(w, `{"message":{"role":"assistant","content":"Hel"},"done":false}`)
			fmt.Fprintln(w, `{"message":{"role":"assistant","content":"lo","tool_calls":[{"function":{"name":"glob","arguments":{"pattern":"*.go"}}}]},"done":false}`)
			fmt.Fprintln(w, `{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":3}`)
		}
	}))
	defer srv.Close()
	p := ollama.New(ollama.Config{Name: "thinai", Type: "thinai", BaseURL: srv.URL})
	resp, err := p.Chat(context.Background(), llm.Request{Model: "loaded", Messages: []llm.Message{llm.UserText("hi")}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if resp.Message.Text() != "Hello" || resp.StopReason != llm.StopToolUse || resp.Usage.InputTokens != 12 {
		t.Fatalf("resp = %+v", resp)
	}
	if s, ok := ollama.Probe(context.Background(), http.DefaultClient, srv.URL); !ok || !s.Thinai {
		t.Fatal("probe should detect Thinai")
	}
}

func TestNormalizeBaseURL(t *testing.T) {
	for in, want := range map[string]string{
		"192.168.1.36":              "http://192.168.1.36:11434",
		"host:8080":                 "http://host:8080",
		"http://phone.local:11434/": "http://phone.local:11434",
	} {
		if got := ollama.NormalizeBaseURL(in); got != want {
			t.Errorf("%s → %s, want %s", in, got, want)
		}
	}
}

// Anthropic: drive the official SDK against a fake SSE server and check that
// an assistant turn with a thinking block is replayed byte-for-byte.
func TestAnthropicStreamingAndVerbatimReplay(t *testing.T) {
	var bodies []map[string]any
	var headers []http.Header
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]any
		_ = json.NewDecoder(r.Body).Decode(&body)
		bodies = append(bodies, body)
		headers = append(headers, r.Header.Clone())
		w.Header().Set("Content-Type", "text/event-stream")
		events := []string{
			`event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5","content":[],"stop_reason":null,"usage":{"input_tokens":50,"output_tokens":1,"cache_read_input_tokens":20}}}`,
			`event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}`,
			`event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan it"}}`,
			`event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG123"}}`,
			`event: content_block_stop
data: {"type":"content_block_stop","index":0}`,
			`event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}`,
			`event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading."}}`,
			`event: content_block_stop
data: {"type":"content_block_stop","index":1}`,
			`event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}`,
			`event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"main.go\"}"}}`,
			`event: content_block_stop
data: {"type":"content_block_stop","index":2}`,
			`event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":30}}`,
			`event: message_stop
data: {"type":"message_stop"}`,
		}
		for _, e := range events {
			fmt.Fprint(w, e+"\n\n")
		}
	}))
	defer srv.Close()

	p := anthropic.New(anthropic.Config{Name: "anthropic", BaseURL: srv.URL, APIKey: "k"})
	var thinking, text strings.Builder
	req := llm.Request{Model: "claude-opus-5-5", System: "sys", MaxTokens: 1000, Effort: "medium",
		Messages: []llm.Message{llm.UserText("hi")},
		Tools:    []llm.ToolSpec{{Name: "read_file", Description: "read", Schema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}`)}},
	}
	resp, err := p.Chat(context.Background(), req, func(ev llm.StreamEvent) {
		thinking.WriteString(ev.Thinking)
		text.WriteString(ev.Text)
	})
	if err != nil {
		t.Fatal(err)
	}
	calls := resp.Message.ToolCalls()
	if len(calls) != 1 || calls[0].ID != "toolu_1" || string(calls[0].Input) != `{"path":"main.go"}` {
		t.Fatalf("calls = %+v", calls)
	}
	if resp.StopReason != llm.StopToolUse || thinking.String() != "plan it" || text.String() != "Reading." {
		t.Fatalf("stop=%s thinking=%q text=%q", resp.StopReason, thinking.String(), text.String())
	}
	if resp.Usage.InputTokens != 50 || resp.Usage.OutputTokens != 30 || resp.Usage.CacheReadTokens != 20 {
		t.Fatalf("usage = %+v", resp.Usage)
	}
	if resp.Message.Native == nil || !strings.Contains(string(resp.Message.Native.Raw), "SIG123") {
		t.Fatalf("native raw missing signature: %v", resp.Message.Native)
	}

	// Request shape: adaptive thinking, effort, eager tool streaming, betas.
	b0 := bodies[0]
	th := b0["thinking"].(map[string]any)
	if th["type"] != "adaptive" || b0["output_config"].(map[string]any)["effort"] != "medium" {
		t.Fatalf("thinking/effort = %v %v", b0["thinking"], b0["output_config"])
	}
	if cc, ok := b0["cache_control"].(map[string]any); !ok || cc["type"] != "ephemeral" {
		t.Fatalf("top-level cache_control missing: %v", b0["cache_control"])
	}
	tool := b0["tools"].([]any)[0].(map[string]any)
	if tool["eager_input_streaming"] != true || tool["input_schema"].(map[string]any)["additionalProperties"] != false {
		t.Fatalf("tool = %v", tool)
	}
	if !strings.Contains(headers[0].Get("anthropic-beta"), "thinking-binding-controls") {
		t.Fatalf("betas = %q", headers[0].Get("anthropic-beta"))
	}

	// Second request replays the assistant turn verbatim.
	req.Messages = append(req.Messages, resp.Message, llm.Message{Role: llm.RoleUser, Parts: []llm.Part{
		{Type: llm.PartToolResult, ToolResult: &llm.ToolResult{CallID: "toolu_1", Name: "read_file", Content: "package main"}},
	}})
	if _, err := p.Chat(context.Background(), req, nil); err != nil {
		t.Fatal(err)
	}
	msgs := bodies[1]["messages"].([]any)
	asst := msgs[1].(map[string]any)["content"].([]any)
	first := asst[0].(map[string]any)
	if first["type"] != "thinking" || first["signature"] != "SIG123" || first["thinking"] != "plan it" {
		t.Fatalf("thinking block not replayed verbatim: %v", first)
	}
	res := msgs[2].(map[string]any)["content"].([]any)[0].(map[string]any)
	if res["type"] != "tool_result" || res["tool_use_id"] != "toolu_1" {
		t.Fatalf("tool result = %v", res)
	}
}
