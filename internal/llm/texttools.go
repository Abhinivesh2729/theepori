package llm

import (
	"context"
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
)

// TextTools adapts a provider for models without reliable native tool calling
// (typically small local models): tools are described in the system prompt and
// calls are parsed from <tool_call>{...}</tool_call> blocks in the output.
func TextTools(p Provider) Provider { return &textTools{inner: p} }

type textTools struct{ inner Provider }

func (t *textTools) Name() string { return t.inner.Name() }

func (t *textTools) ListModels(ctx context.Context) ([]ModelInfo, error) {
	return t.inner.ListModels(ctx)
}

func (t *textTools) Chat(ctx context.Context, req Request, onEvent StreamFunc) (*Response, error) {
	req.System = req.System + "\n\n" + TextToolPrompt(req.Tools)
	req.Tools = nil
	msgs := make([]Message, 0, len(req.Messages))
	for _, m := range req.Messages {
		msgs = append(msgs, flattenTools(m))
	}
	req.Messages = msgs

	filter := &tagFilter{open: "<tool_call>", close: "</tool_call>"}
	resp, err := t.inner.Chat(ctx, req, func(ev StreamEvent) {
		if ev.Text != "" {
			ev.Text = filter.Feed(ev.Text)
			if ev.Text == "" {
				return
			}
		}
		if onEvent != nil {
			onEvent(ev)
		}
	})
	if err != nil {
		return nil, err
	}
	text, calls := ParseTextToolCalls(resp.Message.Text())
	parts := []Part{}
	if strings.TrimSpace(text) != "" {
		parts = append(parts, Part{Type: PartText, Text: text})
	}
	for i := range calls {
		parts = append(parts, Part{Type: PartToolCall, ToolCall: &calls[i]})
	}
	resp.Message = Message{Role: RoleAssistant, Parts: parts}
	if len(calls) > 0 {
		resp.StopReason = StopToolUse
	}
	return resp, nil
}

// TextToolPrompt renders tool specs and the call protocol.
func TextToolPrompt(tools []ToolSpec) string {
	var b strings.Builder
	b.WriteString("# Tool calling\nYou can call tools. To call one, output a block exactly like:\n")
	b.WriteString("<tool_call>\n{\"name\": \"read_file\", \"input\": {\"path\": \"main.go\"}}\n</tool_call>\n")
	b.WriteString("You may output several blocks. After your tool calls, STOP and wait: results arrive in the next message as <tool_result> blocks. ")
	b.WriteString("Never invent tool results. When the task is done, answer without any tool_call block.\n\n## Available tools\n")
	for _, s := range tools {
		fmt.Fprintf(&b, "\n### %s\n%s\nInput JSON schema: %s\n", s.Name, s.Description, compactJSON(s.Schema))
	}
	return b.String()
}

func flattenTools(m Message) Message {
	out := Message{Role: m.Role}
	var b strings.Builder
	for _, p := range m.Parts {
		switch p.Type {
		case PartText:
			b.WriteString(p.Text)
		case PartToolCall:
			fmt.Fprintf(&b, "\n<tool_call>\n{\"name\": %q, \"input\": %s}\n</tool_call>\n", p.ToolCall.Name, compactJSON(p.ToolCall.Input))
		case PartToolResult:
			r := p.ToolResult
			status := "ok"
			if r.IsError {
				status = "error"
			}
			fmt.Fprintf(&b, "<tool_result name=%q status=%q>\n%s\n</tool_result>\n", r.Name, status, r.Content)
		}
	}
	out.Parts = []Part{{Type: PartText, Text: b.String()}}
	return out
}

var toolCallRe = regexp.MustCompile("(?s)<tool_call>\\s*(.*?)\\s*</tool_call>|```tool_call\\s*(.*?)```")

// ParseTextToolCalls extracts tool calls and returns the remaining prose.
func ParseTextToolCalls(text string) (string, []ToolCall) {
	var calls []ToolCall
	rest := toolCallRe.ReplaceAllStringFunc(text, func(block string) string {
		m := toolCallRe.FindStringSubmatch(block)
		body := m[1]
		if body == "" {
			body = m[2]
		}
		body = strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(strings.TrimSpace(body), "```json"), "```"))
		var raw struct {
			Name      string          `json:"name"`
			Input     json.RawMessage `json:"input"`
			Arguments json.RawMessage `json:"arguments"`
		}
		if err := json.Unmarshal([]byte(body), &raw); err != nil || raw.Name == "" {
			return block // leave malformed blocks visible to the model on replay
		}
		in := raw.Input
		if len(in) == 0 {
			in = raw.Arguments
		}
		if len(in) == 0 {
			in = json.RawMessage("{}")
		}
		calls = append(calls, ToolCall{ID: fmt.Sprintf("call_%d", len(calls)+1), Name: raw.Name, Input: in})
		return ""
	})
	// Unclosed trailing <tool_call> (model hit a stop): try to salvage it.
	if i := strings.LastIndex(rest, "<tool_call>"); i >= 0 && !strings.Contains(rest[i:], "</tool_call>") {
		if _, c := ParseTextToolCalls(rest[i:] + "</tool_call>"); len(c) == 1 {
			c[0].ID = fmt.Sprintf("call_%d", len(calls)+1)
			calls = append(calls, c[0])
			rest = rest[:i]
		}
	}
	return strings.TrimSpace(rest), calls
}

func compactJSON(raw json.RawMessage) string {
	var v any
	if json.Unmarshal(raw, &v) != nil {
		return string(raw)
	}
	b, _ := json.Marshal(v)
	return string(b)
}

// tagFilter suppresses streamed text between open and close tags.
type tagFilter struct {
	open, close string
	inside      bool
	pending     string
}

func (f *tagFilter) Feed(s string) string {
	f.pending += s
	var out strings.Builder
	for {
		tag := f.open
		if f.inside {
			tag = f.close
		}
		if i := strings.Index(f.pending, tag); i >= 0 {
			if !f.inside {
				out.WriteString(f.pending[:i])
			}
			f.pending = f.pending[i+len(tag):]
			f.inside = !f.inside
			continue
		}
		// Keep a possible partial tag at the end.
		keep := 0
		for k := len(tag) - 1; k > 0; k-- {
			if strings.HasSuffix(f.pending, tag[:k]) {
				keep = k
				break
			}
		}
		if !f.inside {
			out.WriteString(f.pending[:len(f.pending)-keep])
		}
		f.pending = f.pending[len(f.pending)-keep:]
		return out.String()
	}
}
