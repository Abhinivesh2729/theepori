package permission

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/secrets"
)

// AuditEntry records one tool action and its outcome.
type AuditEntry struct {
	Time       time.Time `json:"time"`
	Session    string    `json:"session"`
	Tool       string    `json:"tool"`
	Risk       string    `json:"risk"`
	Summary    string    `json:"summary"`
	Decision   string    `json:"decision"` // auto | approved | always | denied | policy
	ExitCode   *int      `json:"exit_code,omitempty"`
	DurationMs int64     `json:"duration_ms,omitempty"`
	Error      string    `json:"error,omitempty"`
}

// AuditLog appends JSON lines to a local file. Secrets are redacted.
type AuditLog struct {
	mu     sync.Mutex
	path   string
	redact *secrets.Redactor
}

func NewAuditLog(path string, r *secrets.Redactor) *AuditLog {
	return &AuditLog{path: path, redact: r}
}

func (a *AuditLog) Path() string { return a.path }

func (a *AuditLog) Log(e AuditEntry) {
	if a == nil {
		return
	}
	if e.Time.IsZero() {
		e.Time = time.Now()
	}
	e.Summary = a.redact.Redact(e.Summary)
	e.Error = a.redact.Redact(e.Error)
	b, err := json.Marshal(e)
	if err != nil {
		return
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if err := os.MkdirAll(filepath.Dir(a.path), 0o700); err != nil {
		return
	}
	f, err := os.OpenFile(a.path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return
	}
	defer f.Close()
	f.Write(append(b, '\n'))
}
