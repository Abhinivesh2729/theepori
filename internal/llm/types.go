// Package llm defines the provider-neutral conversation model and the
// Provider interface every LLM adapter implements.
package llm

import (
	"context"
	"encoding/json"
	"strings"
)

type Role string

const (
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
)

type PartType string

const (
	PartText       PartType = "text"
	PartThinking   PartType = "thinking"
	PartToolCall   PartType = "tool_call"
	PartToolResult PartType = "tool_result"
)

// Part is one content block of a message.
type Part struct {
	Type       PartType    `json:"type"`
	Text       string      `json:"text,omitempty"`
	ToolCall   *ToolCall   `json:"tool_call,omitempty"`
	ToolResult *ToolResult `json:"tool_result,omitempty"`
}

type ToolCall struct {
	ID    string          `json:"id"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

type ToolResult struct {
	CallID  string `json:"call_id"`
	Name    string `json:"name"`
	Content string `json:"content"`
	IsError bool   `json:"is_error,omitempty"`
}

// Native carries a provider's verbatim wire form of an assistant message so it
// can be replayed byte-for-byte (required for Anthropic thinking blocks).
type Native struct {
	Provider string          `json:"provider"`
	Model    string          `json:"model"`
	Raw      json.RawMessage `json:"raw"`
}

type Message struct {
	Role    Role    `json:"role"`
	Parts   []Part  `json:"parts"`
	Native  *Native `json:"native,omitempty"`
	Summary bool    `json:"summary,omitempty"` // produced by compaction
}

func UserText(text string) Message {
	return Message{Role: RoleUser, Parts: []Part{{Type: PartText, Text: text}}}
}

// Text concatenates the visible text parts.
func (m Message) Text() string {
	var b strings.Builder
	for _, p := range m.Parts {
		if p.Type == PartText {
			b.WriteString(p.Text)
		}
	}
	return b.String()
}

func (m Message) ToolCalls() []ToolCall {
	var out []ToolCall
	for _, p := range m.Parts {
		if p.Type == PartToolCall && p.ToolCall != nil {
			out = append(out, *p.ToolCall)
		}
	}
	return out
}

type ToolSpec struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Schema      json.RawMessage `json:"schema"`
}

type Request struct {
	Model     string
	System    string
	Messages  []Message
	Tools     []ToolSpec
	MaxTokens int
	Effort    string // anthropic: low|medium|high|xhigh|max
}

type Usage struct {
	InputTokens      int `json:"input_tokens"`
	OutputTokens     int `json:"output_tokens"`
	CacheReadTokens  int `json:"cache_read_tokens,omitempty"`
	CacheWriteTokens int `json:"cache_write_tokens,omitempty"`
}

func (u *Usage) Add(o Usage) {
	u.InputTokens += o.InputTokens
	u.OutputTokens += o.OutputTokens
	u.CacheReadTokens += o.CacheReadTokens
	u.CacheWriteTokens += o.CacheWriteTokens
}

// Stop reasons normalised across providers.
const (
	StopEnd       = "end"
	StopToolUse   = "tool_use"
	StopMaxTokens = "max_tokens"
	StopRefusal   = "refusal"
	StopOther     = "other"
)

type Response struct {
	Message    Message
	StopReason string
	StopDetail string // e.g. refusal category / explanation
	Usage      Usage
	Model      string
}

// StreamEvent is emitted while a response is generated.
type StreamEvent struct {
	Text     string // visible text delta
	Thinking string // reasoning summary delta
	ToolName string // a tool call started streaming
}

type StreamFunc func(StreamEvent)

type ModelInfo struct {
	ID            string
	ContextWindow int
	MaxOutput     int
	InputPrice    float64 // USD per 1M tokens
	OutputPrice   float64
	CacheRead     float64
	Tools         ToolMode
}

// ToolMode says how a model receives tools.
type ToolMode string

const (
	ToolsNative ToolMode = "native" // provider tool-calling API
	ToolsText   ToolMode = "text"   // tools described in the prompt, calls parsed from text
)

type Provider interface {
	Name() string
	Chat(ctx context.Context, req Request, onEvent StreamFunc) (*Response, error)
	ListModels(ctx context.Context) ([]ModelInfo, error)
}
