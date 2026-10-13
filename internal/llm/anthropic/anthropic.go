// Package anthropic adapts the Claude Messages API (via the official Go SDK)
// to the llm.Provider interface.
package anthropic

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"time"

	sdk "github.com/anthropics/anthropic-sdk-go"
	"github.com/anthropics/anthropic-sdk-go/option"
	"github.com/anthropics/anthropic-sdk-go/packages/param"
	"github.com/anthropics/anthropic-sdk-go/shared/constant"

	"github.com/Abhinivesh2729/theepori/internal/llm"
)

type Config struct {
	Name    string
	BaseURL string // empty = Claude API
	APIKey  string
	Timeout time.Duration
	// RefusalFallback opts into server-side fallback when a safety classifier
	// declines a request (Claude API only).
	RefusalFallback bool
}

type Provider struct {
	cfg    Config
	client sdk.Client
}

func New(cfg Config) *Provider {
	opts := []option.RequestOption{option.WithAPIKey(cfg.APIKey), option.WithMaxRetries(0)}
	if cfg.BaseURL != "" {
		opts = append(opts, option.WithBaseURL(cfg.BaseURL))
	}
	if cfg.Timeout > 0 {
		opts = append(opts, option.WithRequestTimeout(cfg.Timeout))
	}
	return &Provider{cfg: cfg, client: sdk.NewClient(opts...)}
}

func (p *Provider) Name() string { return p.cfg.Name }

// Models with adaptive thinking + effort (Claude 4.6 and later).
var adaptiveRe = regexp.MustCompile(`^claude-(opus|sonnet|haiku|fable|mythos)-(5|4-[678])`)

// Models that accept server-side refusal fallback "default".
var fallbackRe = regexp.MustCompile(`^claude-(opus-5|sonnet-5-5|fable-5)`)

func (p *Provider) Chat(ctx context.Context, req llm.Request, onEvent llm.StreamFunc) (*llm.Response, error) {
	params := sdk.BetaMessageNewParams{
		Model:     sdk.Model(req.Model),
		MaxTokens: int64(req.MaxTokens),
		Messages:  convertMessages(req.Messages),
		// Top-level automatic prompt caching: system and tools are frozen per
		// session and history is append-only, so the prefix stays cacheable.
		CacheControl: sdk.BetaCacheControlEphemeralParam{},
	}
	if req.System != "" {
		params.System = []sdk.BetaTextBlockParam{{Text: req.System}}
	}
	for _, t := range req.Tools {
		params.Tools = append(params.Tools, convertTool(t))
	}
	var betas []sdk.AnthropicBeta
	if adaptiveRe.MatchString(req.Model) {
		params.Thinking = sdk.BetaThinkingConfigParamUnion{OfAdaptive: &sdk.BetaThinkingConfigAdaptiveParam{
			Display: sdk.BetaThinkingConfigAdaptiveDisplaySummarized,
			// Degrade instead of failing if an old thinking block no longer
			// matches its prefix (e.g. a session resumed after a tp upgrade).
			BlockBinding: sdk.BetaThinkingBlockBindingParam{PrefixMismatchBehavior: "drop_block"},
		}}
		betas = append(betas, sdk.AnthropicBetaThinkingBindingControls2026_08_01)
		if req.Effort != "" {
			params.OutputConfig = sdk.BetaOutputConfigParam{Effort: sdk.BetaOutputConfigEffort(req.Effort)}
		}
	}
	if p.cfg.RefusalFallback && p.cfg.BaseURL == "" && fallbackRe.MatchString(req.Model) {
		params.Fallbacks = sdk.BetaFallbacksParamUnion{OfDefault: constant.ValueOf[constant.Default]()}
		betas = append(betas, sdk.AnthropicBetaServerSideFallback2026_07_01)
	}
	params.Betas = betas

	stream := p.client.Beta.Messages.NewStreaming(ctx, params)
	defer stream.Close()
	msg := sdk.BetaMessage{}
	for stream.Next() {
		ev := stream.Current()
		if err := msg.Accumulate(ev); err != nil {
			return nil, &llm.Error{Provider: p.Name(), Kind: llm.ErrServer, Message: "stream: " + err.Error()}
		}
		if onEvent == nil {
			continue
		}
		switch e := ev.AsAny().(type) {
		case sdk.BetaRawContentBlockStartEvent:
			if e.ContentBlock.Type == "tool_use" {
				onEvent(llm.StreamEvent{ToolName: e.ContentBlock.Name})
			}
		case sdk.BetaRawContentBlockDeltaEvent:
			switch d := e.Delta.AsAny().(type) {
			case sdk.BetaTextDelta:
				onEvent(llm.StreamEvent{Text: d.Text})
			case sdk.BetaThinkingDelta:
				onEvent(llm.StreamEvent{Thinking: d.Thinking})
			}
		}
	}
	if err := stream.Err(); err != nil {
		return nil, p.classify(err)
	}
	return p.toResponse(msg)
}

func (p *Provider) classify(err error) error {
	var apiErr *sdk.Error
	if errors.As(err, &apiErr) {
		var hdr = apiErr.Response
		e := llm.FromStatus(p.Name(), apiErr.StatusCode, apiErr.RawJSON(), nil)
		if hdr != nil {
			e = llm.FromStatus(p.Name(), apiErr.StatusCode, apiErr.RawJSON(), hdr.Header)
		}
		if apiErr.StatusCode == 529 || apiErr.Type() == "overloaded_error" {
			e.Kind = llm.ErrServer
			e.Hint = "Anthropic is overloaded; retrying or use fallback_models"
		}
		if e.Kind == llm.ErrAuth {
			e.Hint = "set a valid key in the env var configured for provider " + p.Name() + " (see `tp auth`)"
		}
		return e
	}
	base := p.cfg.BaseURL
	if base == "" {
		base = "https://api.anthropic.com"
	}
	return llm.FromTransport(p.Name(), base, err)
}

func (p *Provider) toResponse(msg sdk.BetaMessage) (*llm.Response, error) {
	out := llm.Message{Role: llm.RoleAssistant}
	for _, b := range msg.Content {
		switch b.Type {
		case "text":
			out.Parts = append(out.Parts, llm.Part{Type: llm.PartText, Text: b.Text})
		case "thinking", "redacted_thinking":
			out.Parts = append(out.Parts, llm.Part{Type: llm.PartThinking, Text: b.Thinking})
		case "tool_use":
			in := json.RawMessage(b.Input)
			if len(in) == 0 {
				in = json.RawMessage("{}")
			}
			out.Parts = append(out.Parts, llm.Part{Type: llm.PartToolCall, ToolCall: &llm.ToolCall{ID: b.ID, Name: b.Name, Input: in}})
		}
	}
	raw, err := json.Marshal(msg.ToParam())
	if err == nil {
		out.Native = &llm.Native{Provider: "anthropic", Model: string(msg.Model), Raw: raw}
	}
	resp := &llm.Response{
		Message: out,
		Model:   string(msg.Model),
		Usage: llm.Usage{
			InputTokens:      int(msg.Usage.InputTokens),
			OutputTokens:     int(msg.Usage.OutputTokens),
			CacheReadTokens:  int(msg.Usage.CacheReadInputTokens),
			CacheWriteTokens: int(msg.Usage.CacheCreationInputTokens),
		},
	}
	switch msg.StopReason {
	case "end_turn", "stop_sequence":
		resp.StopReason = llm.StopEnd
	case "tool_use":
		resp.StopReason = llm.StopToolUse
	case "max_tokens", "model_context_window_exceeded":
		resp.StopReason = llm.StopMaxTokens
	case "refusal":
		resp.StopReason = llm.StopRefusal
		resp.StopDetail = strings.TrimSpace(fmt.Sprintf("%s %s", msg.StopDetails.Category, msg.StopDetails.Explanation))
	default:
		resp.StopReason = llm.StopOther
	}
	return resp, nil
}

func convertTool(t llm.ToolSpec) sdk.BetaToolUnionParam {
	var schema map[string]any
	_ = json.Unmarshal(t.Schema, &schema)
	in := sdk.BetaToolInputSchemaParam{ExtraFields: map[string]any{}}
	for k, v := range schema {
		switch k {
		case "type":
		case "properties":
			in.Properties = v
		case "required":
			if arr, ok := v.([]any); ok {
				for _, s := range arr {
					in.Required = append(in.Required, fmt.Sprint(s))
				}
			}
		default:
			in.ExtraFields[k] = v
		}
	}
	return sdk.BetaToolUnionParam{OfTool: &sdk.BetaToolParam{
		Name:        t.Name,
		Description: sdk.String(t.Description),
		InputSchema: in,
		// Large inputs (file contents) stream as generated; tools validate
		// their own input, so a truncated payload becomes an error result.
		EagerInputStreaming: sdk.Bool(true),
	}}
}

func convertMessages(msgs []llm.Message) []sdk.BetaMessageParam {
	out := make([]sdk.BetaMessageParam, 0, len(msgs))
	for _, m := range msgs {
		if m.Role == llm.RoleAssistant && m.Native != nil && m.Native.Provider == "anthropic" {
			// Replay verbatim: thinking blocks must come back unchanged.
			out = append(out, param.Override[sdk.BetaMessageParam](json.RawMessage(m.Native.Raw)))
			continue
		}
		var blocks []sdk.BetaContentBlockParamUnion
		// tool_result blocks must lead a user message.
		for _, p := range m.Parts {
			if p.Type == llm.PartToolResult {
				r := p.ToolResult
				content := r.Content
				if content == "" {
					content = "(no output)"
				}
				blocks = append(blocks, sdk.NewBetaToolResultBlock(r.CallID, content, r.IsError))
			}
		}
		for _, p := range m.Parts {
			switch p.Type {
			case llm.PartText:
				if strings.TrimSpace(p.Text) != "" {
					blocks = append(blocks, sdk.NewBetaTextBlock(p.Text))
				}
			case llm.PartToolCall:
				in := p.ToolCall.Input
				if len(in) == 0 {
					in = json.RawMessage("{}")
				}
				blocks = append(blocks, sdk.NewBetaToolUseBlock(p.ToolCall.ID, in, p.ToolCall.Name))
			}
		}
		if len(blocks) == 0 {
			blocks = append(blocks, sdk.NewBetaTextBlock("(empty)"))
		}
		if m.Role == llm.RoleAssistant {
			out = append(out, sdk.BetaMessageParam{Role: sdk.BetaMessageParamRoleAssistant, Content: blocks})
		} else {
			out = append(out, sdk.NewBetaUserMessage(blocks...))
		}
	}
	return out
}

func (p *Provider) ListModels(ctx context.Context) ([]llm.ModelInfo, error) {
	pager := p.client.Models.ListAutoPaging(ctx, sdk.ModelListParams{})
	var out []llm.ModelInfo
	for pager.Next() {
		m := pager.Current()
		info := llm.LookupModel("anthropic", m.ID)
		if m.MaxInputTokens > 0 {
			info.ContextWindow = int(m.MaxInputTokens)
		}
		if m.MaxTokens > 0 {
			info.MaxOutput = int(m.MaxTokens)
		}
		out = append(out, info)
	}
	if err := pager.Err(); err != nil {
		return nil, p.classify(err)
	}
	return out, nil
}
