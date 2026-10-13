package llm

import (
	"context"
	"fmt"
	"math/rand/v2"
	"time"
)

// Target is a resolved provider+model pair.
type Target struct {
	Ref      string
	Provider Provider
	Model    ModelInfo
}

// Client adds retries and model fallback on top of providers.
type Client struct {
	Targets    []Target // primary first, then fallbacks
	MaxRetries int
	Notify     func(string) // user-visible status lines
}

// Chat sends req to the primary target, retrying transient failures and
// falling through to fallback targets. It returns the target that answered.
func (c *Client) Chat(ctx context.Context, req Request, onEvent StreamFunc) (*Response, Target, error) {
	if len(c.Targets) == 0 {
		return nil, Target{}, &Error{Provider: "tp", Kind: ErrConfig, Message: "no model configured", Hint: "run `tp models use <provider/model>`"}
	}
	var lastErr error
	for i, t := range c.Targets {
		if i > 0 {
			c.notify(fmt.Sprintf("falling back to %s", t.Ref))
		}
		resp, err := c.chatWithRetry(ctx, t, req, onEvent)
		if err == nil {
			return resp, t, nil
		}
		lastErr = err
		e, ok := AsError(err)
		if !ok || !e.Fallbackable() || ctx.Err() != nil {
			return nil, t, err
		}
	}
	return nil, Target{}, lastErr
}

func (c *Client) chatWithRetry(ctx context.Context, t Target, req Request, onEvent StreamFunc) (*Response, error) {
	req.Model = t.Model.ID
	if req.MaxTokens == 0 || (t.Model.MaxOutput > 0 && req.MaxTokens > t.Model.MaxOutput) {
		req.MaxTokens = t.Model.MaxOutput
	}
	p := t.Provider
	if t.Model.Tools == ToolsText && len(req.Tools) > 0 {
		p = TextTools(p)
	}
	for attempt := 0; ; attempt++ {
		streamed := false
		wrapped := func(ev StreamEvent) {
			streamed = true
			if onEvent != nil {
				onEvent(ev)
			}
		}
		resp, err := p.Chat(ctx, req, wrapped)
		if err == nil {
			return resp, nil
		}
		e, ok := AsError(err)
		// Never retry once output reached the user: it would be duplicated.
		if !ok || !e.Retryable() || streamed || attempt >= c.MaxRetries {
			return nil, err
		}
		delay := e.RetryAfter
		if delay == 0 {
			delay = time.Duration(1<<attempt) * time.Second
			delay += time.Duration(rand.IntN(500)) * time.Millisecond
		}
		if delay > 30*time.Second {
			delay = 30 * time.Second
		}
		c.notify(fmt.Sprintf("%s (retry %d/%d in %s)", e.Kind, attempt+1, c.MaxRetries, delay.Round(100*time.Millisecond)))
		select {
		case <-ctx.Done():
			return nil, FromTransport(t.Provider.Name(), "", ctx.Err())
		case <-time.After(delay):
		}
	}
}

func (c *Client) notify(s string) {
	if c.Notify != nil {
		c.Notify(s)
	}
}
