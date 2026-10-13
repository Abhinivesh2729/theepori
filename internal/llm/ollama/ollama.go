// Package ollama speaks the Ollama /api/chat protocol, which the Thinai
// Android app serves on the local network (default port 11434).
package ollama

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/llm"
)

const DefaultPort = 11434

type Config struct {
	Name    string
	Type    string // "thinai" or "ollama"
	BaseURL string // http://host:11434
	APIKey  string // optional bearer token
	Timeout time.Duration
	NumCtx  int // context window requested from the server (0 = server default)
}

type Provider struct {
	cfg Config
	hc  *http.Client
}

func New(cfg Config) *Provider {
	cfg.BaseURL = NormalizeBaseURL(cfg.BaseURL)
	if cfg.Timeout == 0 {
		cfg.Timeout = 5 * time.Minute // phone inference: slow first token
	}
	return &Provider{cfg: cfg, hc: llm.HTTPClient(cfg.Timeout)}
}

// NormalizeBaseURL accepts "192.168.1.36", "host:8080" or a full URL.
func NormalizeBaseURL(s string) string {
	s = strings.TrimRight(strings.TrimSpace(s), "/")
	if s == "" {
		return ""
	}
	if !strings.Contains(s, "://") {
		s = "http://" + s
	}
	rest := s[strings.Index(s, "://")+3:]
	if !strings.Contains(rest, ":") && !strings.HasPrefix(rest, "[") {
		s = fmt.Sprintf("%s:%d", s, DefaultPort)
	}
	return s
}

func (p *Provider) Name() string { return p.cfg.Name }

func (p *Provider) headers() map[string]string {
	if p.cfg.APIKey == "" {
		return nil
	}
	return map[string]string{"Authorization": "Bearer " + p.cfg.APIKey}
}

type wireMsg struct {
	Role      string     `json:"role"`
	Content   string     `json:"content"`
	ToolCalls []wireCall `json:"tool_calls,omitempty"`
	ToolName  string     `json:"tool_name,omitempty"`
}

type wireCall struct {
	ID       string `json:"id,omitempty"`
	Function struct {
		Name      string          `json:"name"`
		Arguments json.RawMessage `json:"arguments"`
	} `json:"function"`
}

func convertMessages(system string, msgs []llm.Message) []wireMsg {
	var out []wireMsg
	if system != "" {
		out = append(out, wireMsg{Role: "system", Content: system})
	}
	for _, m := range msgs {
		if m.Role == llm.RoleAssistant {
			w := wireMsg{Role: "assistant", Content: m.Text()}
			for _, c := range m.ToolCalls() {
				var wc wireCall
				wc.ID, wc.Function.Name, wc.Function.Arguments = c.ID, c.Name, c.Input
				w.ToolCalls = append(w.ToolCalls, wc)
			}
			out = append(out, w)
			continue
		}
		for _, p := range m.Parts {
			if p.Type == llm.PartToolResult {
				out = append(out, wireMsg{Role: "tool", Content: p.ToolResult.Content, ToolName: p.ToolResult.Name})
			}
		}
		if t := m.Text(); strings.TrimSpace(t) != "" {
			out = append(out, wireMsg{Role: "user", Content: t})
		}
	}
	return out
}

func (p *Provider) Chat(ctx context.Context, req llm.Request, onEvent llm.StreamFunc) (*llm.Response, error) {
	if p.cfg.BaseURL == "" {
		return nil, &llm.Error{Provider: p.Name(), Kind: llm.ErrConfig, Message: "no base_url",
			Hint: "set providers." + p.Name() + ".base_url or run `tp providers discover`"}
	}
	model := req.Model
	if model == "" || model == "loaded" {
		model = p.loadedModel(ctx)
	}
	body := map[string]any{
		"model":    model,
		"messages": convertMessages(req.System, req.Messages),
		"stream":   true,
	}
	opts := map[string]any{}
	if req.MaxTokens > 0 {
		opts["num_predict"] = req.MaxTokens
	}
	if p.cfg.NumCtx > 0 {
		opts["num_ctx"] = p.cfg.NumCtx
	}
	if len(opts) > 0 {
		body["options"] = opts
	}
	if len(req.Tools) > 0 {
		var tools []map[string]any
		for _, t := range req.Tools {
			tools = append(tools, map[string]any{"type": "function", "function": map[string]any{
				"name": t.Name, "description": t.Description, "parameters": t.Schema}})
		}
		body["tools"] = tools
	}
	resp, err := llm.PostJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/api/chat", p.headers(), body)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var text strings.Builder
	var calls []llm.ToolCall
	out := &llm.Response{Model: model}
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		var c struct {
			Model   string `json:"model"`
			Message struct {
				Content   string     `json:"content"`
				Thinking  string     `json:"thinking"`
				ToolCalls []wireCall `json:"tool_calls"`
			} `json:"message"`
			Done            bool   `json:"done"`
			DoneReason      string `json:"done_reason"`
			PromptEvalCount int    `json:"prompt_eval_count"`
			EvalCount       int    `json:"eval_count"`
			Error           string `json:"error"`
		}
		if err := json.Unmarshal([]byte(line), &c); err != nil {
			continue
		}
		if c.Error != "" {
			return nil, &llm.Error{Provider: p.Name(), Kind: llm.ErrServer, Message: c.Error}
		}
		if c.Message.Content != "" {
			text.WriteString(c.Message.Content)
			if onEvent != nil {
				onEvent(llm.StreamEvent{Text: c.Message.Content})
			}
		}
		if c.Message.Thinking != "" && onEvent != nil {
			onEvent(llm.StreamEvent{Thinking: c.Message.Thinking})
		}
		for _, tc := range c.Message.ToolCalls {
			id := tc.ID
			if id == "" {
				id = fmt.Sprintf("call_%d", len(calls)+1)
			}
			in := tc.Function.Arguments
			// Some servers send arguments as a JSON string.
			var s string
			if json.Unmarshal(in, &s) == nil {
				in = json.RawMessage(s)
			}
			if len(in) == 0 {
				in = json.RawMessage("{}")
			}
			calls = append(calls, llm.ToolCall{ID: id, Name: tc.Function.Name, Input: in})
			if onEvent != nil {
				onEvent(llm.StreamEvent{ToolName: tc.Function.Name})
			}
		}
		if c.Model != "" {
			out.Model = c.Model
		}
		if c.Done {
			out.Usage = llm.Usage{InputTokens: c.PromptEvalCount, OutputTokens: c.EvalCount}
			switch {
			case len(calls) > 0:
				out.StopReason = llm.StopToolUse
			case c.DoneReason == "length":
				out.StopReason = llm.StopMaxTokens
			default:
				out.StopReason = llm.StopEnd
			}
		}
	}
	if err := sc.Err(); err != nil {
		return nil, llm.FromTransport(p.Name(), p.cfg.BaseURL, err)
	}
	msg := llm.Message{Role: llm.RoleAssistant}
	if text.Len() > 0 {
		msg.Parts = append(msg.Parts, llm.Part{Type: llm.PartText, Text: text.String()})
	}
	for i := range calls {
		msg.Parts = append(msg.Parts, llm.Part{Type: llm.PartToolCall, ToolCall: &calls[i]})
	}
	out.Message = msg
	if out.StopReason == "" {
		out.StopReason = llm.StopEnd
	}
	return out, nil
}

// loadedModel returns the model already loaded on the server; swapping models
// on a phone is slow, so tp prefers whatever is resident.
func (p *Provider) loadedModel(ctx context.Context) string {
	var res struct {
		Models []struct {
			Name string `json:"name"`
		} `json:"models"`
	}
	if llm.GetJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/api/ps", p.headers(), &res) == nil && len(res.Models) > 0 {
		return res.Models[0].Name
	}
	return ""
}

func (p *Provider) ListModels(ctx context.Context) ([]llm.ModelInfo, error) {
	if p.cfg.BaseURL == "" {
		return nil, &llm.Error{Provider: p.Name(), Kind: llm.ErrConfig, Message: "no base_url",
			Hint: "run `tp providers discover` or set providers." + p.Name() + ".base_url"}
	}
	var res struct {
		Models []struct {
			Name string `json:"name"`
		} `json:"models"`
	}
	if err := llm.GetJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/api/tags", p.headers(), &res); err != nil {
		return nil, err
	}
	var out []llm.ModelInfo
	for _, m := range res.Models {
		info := llm.LookupModel(p.cfg.Type, m.Name)
		if n := p.contextCap(ctx, m.Name); n > 0 {
			info.ContextWindow = n
		}
		out = append(out, info)
	}
	return out, nil
}

// contextCap reads thinai.context_cap (or <arch>.context_length) from /api/show.
func (p *Provider) contextCap(ctx context.Context, model string) int {
	resp, err := llm.PostJSON(ctx, p.hc, p.Name(), p.cfg.BaseURL+"/api/show", p.headers(), map[string]string{"name": model})
	if err != nil {
		return 0
	}
	defer resp.Body.Close()
	var res struct {
		ModelInfo map[string]any `json:"model_info"`
	}
	if json.NewDecoder(resp.Body).Decode(&res) != nil {
		return 0
	}
	best := 0
	for k, v := range res.ModelInfo {
		f, ok := v.(float64)
		if !ok {
			continue
		}
		if k == "thinai.context_cap" {
			return int(f)
		}
		if strings.HasSuffix(k, ".context_length") {
			best = int(f)
		}
	}
	return best
}
