package llm

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"strconv"
	"time"
)

type ErrKind string

const (
	ErrAuth          ErrKind = "auth"
	ErrRateLimit     ErrKind = "rate_limit"
	ErrTimeout       ErrKind = "timeout"
	ErrNetwork       ErrKind = "network"
	ErrServer        ErrKind = "server"
	ErrBadRequest    ErrKind = "bad_request"
	ErrNotFound      ErrKind = "not_found"
	ErrContextLength ErrKind = "context_length"
	ErrCanceled      ErrKind = "canceled"
	ErrConfig        ErrKind = "config"
)

// Error is the normalised provider error. Hint carries an actionable fix.
type Error struct {
	Provider   string
	Kind       ErrKind
	Status     int
	Message    string
	RetryAfter time.Duration
	Hint       string
}

func (e *Error) Error() string {
	s := fmt.Sprintf("%s: %s", e.Provider, e.Kind)
	if e.Status != 0 {
		s += fmt.Sprintf(" (HTTP %d)", e.Status)
	}
	if e.Message != "" {
		s += ": " + e.Message
	}
	if e.Hint != "" {
		s += "\n  → " + e.Hint
	}
	return s
}

// Retryable reports whether the same request may succeed if retried.
func (e *Error) Retryable() bool {
	switch e.Kind {
	case ErrRateLimit, ErrTimeout, ErrNetwork, ErrServer:
		return true
	}
	return false
}

// Fallbackable reports whether another model might succeed where this one failed.
func (e *Error) Fallbackable() bool {
	return e.Retryable() || e.Kind == ErrNotFound || e.Kind == ErrContextLength
}

func AsError(err error) (*Error, bool) {
	var e *Error
	ok := errors.As(err, &e)
	return e, ok
}

// FromStatus classifies an HTTP error response.
func FromStatus(provider string, status int, body string, header http.Header) *Error {
	e := &Error{Provider: provider, Status: status, Message: trim(body, 400)}
	switch {
	case status == 401 || status == 403:
		e.Kind = ErrAuth
		e.Hint = fmt.Sprintf("check the API key for provider %q (tp auth)", provider)
	case status == 404:
		e.Kind = ErrNotFound
		e.Hint = "check the model id and base_url (tp models)"
	case status == 408:
		e.Kind = ErrTimeout
	case status == 413:
		e.Kind = ErrContextLength
		e.Hint = "request too large; run /compact or start a new session"
	case status == 429:
		e.Kind = ErrRateLimit
		e.Hint = "rate limited; tp retries automatically, or configure fallback_models"
	case status >= 500:
		e.Kind = ErrServer
	default:
		e.Kind = ErrBadRequest
		if containsAny(body, "context length", "context_length", "too long", "maximum context", "prompt is too long") {
			e.Kind = ErrContextLength
			e.Hint = "conversation exceeds the model context; run /compact"
		}
	}
	if header != nil {
		if s := header.Get("retry-after"); s != "" {
			if n, err := strconv.Atoi(s); err == nil {
				e.RetryAfter = time.Duration(n) * time.Second
			}
		}
	}
	return e
}

// FromTransport classifies a transport-level failure.
func FromTransport(provider, baseURL string, err error) *Error {
	if errors.Is(err, context.Canceled) {
		return &Error{Provider: provider, Kind: ErrCanceled, Message: "canceled"}
	}
	var ne net.Error
	if errors.Is(err, context.DeadlineExceeded) || (errors.As(err, &ne) && ne.Timeout()) {
		return &Error{Provider: provider, Kind: ErrTimeout, Message: err.Error(),
			Hint: "increase providers." + provider + ".timeout or check the endpoint"}
	}
	return &Error{Provider: provider, Kind: ErrNetwork, Message: err.Error(),
		Hint: fmt.Sprintf("cannot reach %s; check base_url and network (tp doctor)", baseURL)}
}

func containsAny(s string, subs ...string) bool {
	for _, sub := range subs {
		if len(sub) > 0 && indexFold(s, sub) >= 0 {
			return true
		}
	}
	return false
}

func indexFold(s, sub string) int {
	n, m := len(s), len(sub)
	for i := 0; i+m <= n; i++ {
		match := true
		for j := 0; j < m; j++ {
			a, b := s[i+j], sub[j]
			if 'A' <= a && a <= 'Z' {
				a += 'a' - 'A'
			}
			if 'A' <= b && b <= 'Z' {
				b += 'a' - 'A'
			}
			if a != b {
				match = false
				break
			}
		}
		if match {
			return i
		}
	}
	return -1
}

func trim(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
