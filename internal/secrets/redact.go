// Package secrets redacts credentials from text before it is logged,
// persisted or sent to a model.
package secrets

import (
	"os"
	"regexp"
	"sort"
	"strings"
)

var patterns = []*regexp.Regexp{
	regexp.MustCompile(`sk-ant-[A-Za-z0-9_\-]{20,}`),
	regexp.MustCompile(`sk-(proj-|svcacct-)?[A-Za-z0-9_\-]{20,}`),
	regexp.MustCompile(`gh[pousr]_[A-Za-z0-9]{30,}`),
	regexp.MustCompile(`github_pat_[A-Za-z0-9_]{40,}`),
	regexp.MustCompile(`glpat-[A-Za-z0-9_\-]{20,}`),
	regexp.MustCompile(`xox[baprs]-[A-Za-z0-9\-]{10,}`),
	regexp.MustCompile(`AKIA[0-9A-Z]{16}`),
	regexp.MustCompile(`AIza[0-9A-Za-z_\-]{35}`),
	regexp.MustCompile(`(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----`),
	regexp.MustCompile(`(?i)(api[_-]?key|secret|token|password|passwd)(["']?\s*[:=]\s*["']?)([A-Za-z0-9_\-./+]{12,})`),
}

// Redactor masks known secret values and common credential shapes.
type Redactor struct {
	values []string
}

// New builds a redactor that also masks the current values of envVars.
func New(envVars ...string) *Redactor {
	r := &Redactor{}
	for _, v := range envVars {
		if val := strings.TrimSpace(os.Getenv(v)); len(val) >= 8 {
			r.values = append(r.values, val)
		}
	}
	sort.Slice(r.values, func(i, j int) bool { return len(r.values[i]) > len(r.values[j]) })
	return r
}

func (r *Redactor) Redact(s string) string {
	if r == nil || s == "" {
		return s
	}
	for _, v := range r.values {
		s = strings.ReplaceAll(s, v, "[REDACTED]")
	}
	for i, re := range patterns {
		if i == len(patterns)-1 {
			s = re.ReplaceAllString(s, "${1}${2}[REDACTED]")
			continue
		}
		s = re.ReplaceAllString(s, "[REDACTED]")
	}
	return s
}

// Contains reports whether s appears to hold a secret.
func (r *Redactor) Contains(s string) bool { return r.Redact(s) != s }

// Mask shows only the last 4 characters of a credential.
func Mask(v string) string {
	if len(v) <= 8 {
		return strings.Repeat("•", len(v))
	}
	return v[:3] + "…" + v[len(v)-4:]
}
