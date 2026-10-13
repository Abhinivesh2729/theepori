// Package openai implements the Chat Completions protocol used by OpenAI and
// most OpenAI-compatible endpoints (Groq, OpenRouter, vLLM, LM Studio, ...).
package openai

import (
	"context"
	"encoding/json"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/llm"
)

type Config struct {
	Name    string
	Type    string // "openai" or "openai-compatible" (selects catalog + token field)
	BaseURL string // e.g. https://api.openai.com/v1
	APIKey  string
	Timeout time.Duration
	Headers map[string]string
	// MaxTokensField is "max_completion_tokens" (OpenAI) or "max_tokens".
	MaxTokensField string
}

type Provider struct {
	cfg Config
	hc  *http.Client
}

func New(cfg Config) *Provider {
	cfg.BaseURL = strings.TrimRight(cfg.BaseURL, "/")
	if cfg.MaxTokensField == "" {
		cfg.MaxTokensField = "max_tokens"
		if cfg.Type == "openai" {
			cfg.MaxTokensField = "max_completion_tokens"
		}
	}
	return &Provider{cfg: cfg, hc: llm.HTTPClient(cfg.Timeout)}
}

func (p *Provider) Name() string { return p.cfg.Name }

func (p *Provider) headers() map[string]string {
	h := map[string]string{}
	for k, v := range p.cfg.Headers {
		h[k] = v
	}
	if p.cfg.APIKey != "" {
		h["Authorization"] = "Bearer " + p.cfg.APIKey
	}
	return h
}

type wireMsg struct {
	Role       string     `json:"role"`
	Content    *string    `json:"content"`
	ToolCalls  []wireCall `json:"tool_calls,omitempty"`
	ToolCallID string     `json:"tool_call_id,omitempty"`
}

type wireCall struct {
	Index    *int   `json:"index,omitempty"`
	ID       string `json:"id,omitempty"`
	Type     string `json:"type,omitempty"`
	Function struct {
		Name      string `json:"name,omitempty"`
		Arguments string `json:"arguments"`
	} `json:"function"`
}

type wireTool struct {
	Type     string `json:"type"`
	Function struct {
		Name        string          `json:"name"`
		Description string          `json:"description"`
		Parameters  json.RawMessage `json:"parameters"`
	} `json:"function"`
}

func str(s string) *string { return &s }

func convertMessages(system string, msgs []llm.Message) []wireMsg {
	out := []wireMsg{}
	if system != "" {
		out = append(out, wireMsg{Role: "system", Content: str(system)})
	}
	for _, m := range msgs {
		if m.Role == llm.RoleAssistant {
			w := wireMsg{Role: "assistant"}
			if t := m.Text(); t != "" {
				w.Content = str(t)
			}
			for _, c := range m.ToolCalls() {
				var wc wireCall
				wc.ID, wc.Type = c.ID, "function"
				wc.Function.Name, wc.Function.Arguments = c.Name, string(c.Input)
				w.ToolCalls = append(w.ToolCalls, wc)
			}
			if w.Content == nil && len(w.ToolCalls) == 0 {
				w.Content = str("")
			}
			out = append(out, w)
			continue
		}
		// Tool results become role=tool messages, directly after the assistant turn.
		for _, p := range m.Parts {
			if p.Type == llm.PartToolResult {
				c := p.ToolResult.Content
				if p.ToolResult.IsError {
					c = "ERROR: " + c
				}
				out = append(out, wireMsg{Role: "tool", ToolCallID: p.ToolResult.CallID, Content: str(c)})
			}
		}
		if t := m.Text(); strings.TrimSpace(t) != "" {
			out = append(out, wireMsg{Role: "user", Content: str(t)})
		}
	}
	return out
}

type chunk struct {
	Choices []struct {
		Delta struct {
			Content          string     `json:"content"`
			ReasoningContent string     `json:"reasoning_content"`
			ToolCalls        []wireCall `json:"tool_calls"`
		} `json:"delta"`
		FinishReason string `json:"finish_reason"`
	} `json:"choices"`
	Usage *struct {
		PromptTokens        int `json:"prompt_tokens"`
		CompletionTokens    int `json:"completion_tokens"`
		PromptTokensDetails struct {
			CachedTokens int `json:"cached_tokens"`
		} `json:"prompt_tokens_details"`
	} `json:"usage"`
	Model string `json:"model"`
	Error *struct {
		Message string `json:"message"`
	} `json:"error"`
}

func (p *Provider) Chat(ctx context.Context, req llm.Request, onEvent llm.StreamFunc) (*llm.Response, error) {
	body := map[string]any{
		"model":          req.Model,
		"messages":       convertMessages(req.System, req.Messages),
		"stream":         true,
		"stream_options": map[string]any{"include_usage": true},
	}
	if req.MaxTokens > 0 {
		body[p.cfg.MaxTokensField] = req.MaxTokens
	}
	if len(req.Tools) > 0 {
		var tools []wireTool
		for _, t := range req.Tools {
			var w wireTool
			w.Type = "function"
			w.Function.Name, w.Function.Description, w.Function.Parameters = t.Name, t.Description, t.Schema
			tools = append(tools, w)
		}
		body["tools"] = tools
	}
	resp, err := llm.PostJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/chat/completions", p.headers(), body)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var text strings.Builder
	calls := map[int]*llm.ToolCall{}
	args := map[int]*strings.Builder{}
	out := &llm.Response{Model: req.Model}
	finish := ""
	err = llm.ReadSSE(resp.Body, func(_, data string) error {
		if data == "[DONE]" {
			return nil
		}
		var c chunk
		if err := json.Unmarshal([]byte(data), &c); err != nil {
			return nil // tolerate keep-alives and vendor noise
		}
		if c.Error != nil {
			return &llm.Error{Provider: p.Name(), Kind: llm.ErrServer, Message: c.Error.Message}
		}
		if c.Model != "" {
			out.Model = c.Model
		}
		if c.Usage != nil {
			out.Usage = llm.Usage{
				InputTokens:     c.Usage.PromptTokens - c.Usage.PromptTokensDetails.CachedTokens,
				OutputTokens:    c.Usage.CompletionTokens,
				CacheReadTokens: c.Usage.PromptTokensDetails.CachedTokens,
			}
		}
		for _, ch := range c.Choices {
			if ch.Delta.Content != "" {
				text.WriteString(ch.Delta.Content)
				if onEvent != nil {
					onEvent(llm.StreamEvent{Text: ch.Delta.Content})
				}
			}
			if ch.Delta.ReasoningContent != "" && onEvent != nil {
				onEvent(llm.StreamEvent{Thinking: ch.Delta.ReasoningContent})
			}
			for i, tc := range ch.Delta.ToolCalls {
				idx := i
				if tc.Index != nil {
					idx = *tc.Index
				}
				call, ok := calls[idx]
				if !ok {
					call = &llm.ToolCall{}
					calls[idx], args[idx] = call, &strings.Builder{}
				}
				if tc.ID != "" {
					call.ID = tc.ID
				}
				if tc.Function.Name != "" {
					call.Name = tc.Function.Name
					if onEvent != nil {
						onEvent(llm.StreamEvent{ToolName: call.Name})
					}
				}
				args[idx].WriteString(tc.Function.Arguments)
			}
			if ch.FinishReason != "" {
				finish = ch.FinishReason
			}
		}
		return nil
	})
	if err != nil {
		if e, ok := llm.AsError(err); ok {
			return nil, e
		}
		return nil, llm.FromTransport(p.Name(), p.cfg.BaseURL, err)
	}

	msg := llm.Message{Role: llm.RoleAssistant}
	if text.Len() > 0 {
		msg.Parts = append(msg.Parts, llm.Part{Type: llm.PartText, Text: text.String()})
	}
	idxs := make([]int, 0, len(calls))
	for i := range calls {
		idxs = append(idxs, i)
	}
	sort.Ints(idxs)
	for n, i := range idxs {
		c := calls[i]
		c.Input = json.RawMessage(args[i].String())
		if len(strings.TrimSpace(args[i].String())) == 0 {
			c.Input = json.RawMessage("{}")
		}
		if c.ID == "" {
			c.ID = "call_" + string(rune('a'+n))
		}
		msg.Parts = append(msg.Parts, llm.Part{Type: llm.PartToolCall, ToolCall: c})
	}
	out.Message = msg
	switch finish {
	case "tool_calls", "function_call":
		out.StopReason = llm.StopToolUse
	case "length":
		out.StopReason = llm.StopMaxTokens
	case "content_filter":
		out.StopReason = llm.StopRefusal
		out.StopDetail = "content_filter"
	default:
		out.StopReason = llm.StopEnd
		if len(calls) > 0 {
			out.StopReason = llm.StopToolUse
		}
	}
	return out, nil
}

func (p *Provider) ListModels(ctx context.Context) ([]llm.ModelInfo, error) {
	var res struct {
		Data []struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	if err := llm.GetJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/models", p.headers(), &res); err != nil {
		return nil, err
	}
	var out []llm.ModelInfo
	for _, m := range res.Data {
		out = append(out, llm.LookupModel(p.cfg.Type, m.ID))
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out, nil
}
