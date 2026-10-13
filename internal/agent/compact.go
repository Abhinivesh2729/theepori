package agent

import (
	"context"
	"fmt"
	"strings"

	"github.com/Abhinivesh2729/theepori/internal/llm"
)

// maybeCompact summarises the session when the last prompt used more than
// agent.compact_at of the model's context window.
func (a *Agent) maybeCompact(ctx context.Context) error {
	t := a.Primary()
	if t.Model.ContextWindow == 0 || a.Session.LastContextTokens == 0 {
		return nil
	}
	limit := int(float64(t.Model.ContextWindow) * a.Cfg.Agent.CompactAt)
	if a.Session.LastContextTokens < limit {
		return nil
	}
	a.UI.Notice(fmt.Sprintf("context %d/%d tokens — compacting conversation", a.Session.LastContextTokens, t.Model.ContextWindow))
	return a.Compact(ctx)
}

// Compact replaces the whole history with a summary ("simple compaction"):
// nothing from the old transcript is replayed, so no stale provider state
// (e.g. Claude thinking blocks) survives the boundary.
func (a *Agent) Compact(ctx context.Context) error {
	if len(a.Session.Messages) == 0 {
		return nil
	}
	if a.client == nil {
		if err := a.SetModel(a.Session.Model); err != nil {
			return err
		}
	}
	a.repairHistory()
	transcript := renderTranscript(a.Session.Messages, 2000)
	// Keep the transcript within ~60% of the context window (≈4 chars/token).
	if budget := a.Primary().Model.ContextWindow * 4 * 6 / 10; budget > 0 && len(transcript) > budget {
		transcript = "…(earliest part omitted)…\n" + transcript[len(transcript)-budget:]
	}
	req := llm.Request{
		System:    "You write precise hand-off summaries of software engineering sessions.",
		Messages:  []llm.Message{llm.UserText(compactPrompt + "\n\n<transcript>\n" + transcript + "\n</transcript>")},
		MaxTokens: 8000,
		Effort:    "low",
	}
	resp, target, err := a.client.Chat(ctx, req, nil)
	if err != nil {
		return fmt.Errorf("compaction failed: %w", err)
	}
	a.account(target, resp.Usage)
	summary := strings.TrimSpace(resp.Message.Text())
	if summary == "" {
		return fmt.Errorf("compaction failed: empty summary")
	}

	// A user message waiting for an answer is carried over verbatim.
	pending := ""
	if last := a.Session.Messages[len(a.Session.Messages)-1]; last.Role == llm.RoleUser {
		pending = strings.TrimSpace(last.Text())
	}
	text := "<session-summary>\nThe earlier conversation was compacted. Summary:\n\n" + summary + "\n</session-summary>\n\n"
	if pending != "" {
		text += pending
	} else {
		text += "Continue the task from where it left off."
	}
	msg := llm.UserText(text)
	msg.Summary = true
	a.Session.Messages = []llm.Message{msg}
	a.Session.Compactions++
	a.Session.LastContextTokens = len(text) / 4
	a.save()
	return nil
}

func renderTranscript(msgs []llm.Message, maxResult int) string {
	var b strings.Builder
	for _, m := range msgs {
		for _, p := range m.Parts {
			switch p.Type {
			case llm.PartText:
				fmt.Fprintf(&b, "[%s] %s\n", m.Role, p.Text)
			case llm.PartToolCall:
				fmt.Fprintf(&b, "[assistant → %s] %s\n", p.ToolCall.Name, clipStr(string(p.ToolCall.Input), maxResult))
			case llm.PartToolResult:
				status := ""
				if p.ToolResult.IsError {
					status = " (error)"
				}
				fmt.Fprintf(&b, "[%s result%s] %s\n", p.ToolResult.Name, status, clipStr(p.ToolResult.Content, maxResult))
			}
		}
	}
	return b.String()
}

func clipStr(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n/2] + "\n…\n" + s[len(s)-n/2:]
}
