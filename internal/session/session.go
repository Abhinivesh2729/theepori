// Package session persists conversations, project memory and per-project
// state under the app directory, one folder per project.
package session

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/tools"
)

const (
	StatusActive      = "active"
	StatusInterrupted = "interrupted"
	StatusIdle        = "idle"
)

type Session struct {
	ID          string             `json:"id"`
	Title       string             `json:"title"`
	ProjectRoot string             `json:"project_root"`
	Model       string             `json:"model"`
	Mode        string             `json:"mode"`
	Created     time.Time          `json:"created"`
	Updated     time.Time          `json:"updated"`
	Status      string             `json:"status"`
	System      string             `json:"system"` // frozen at session start
	Messages    []llm.Message      `json:"messages"`
	Usage       llm.Usage          `json:"usage"`
	Cost        float64            `json:"cost_usd"`
	Turns       int                `json:"turns"`
	Compactions int                `json:"compactions,omitempty"`
	Plan        []tools.PlanStep   `json:"plan,omitempty"`
	Tracker     tools.TrackerState `json:"tracker"`
	// LastContextTokens is the prompt size of the latest request.
	LastContextTokens int `json:"last_context_tokens,omitempty"`
}

// Meta is the listing view of a session.
type Meta struct {
	ID       string
	Title    string
	Model    string
	Status   string
	Updated  time.Time
	Turns    int
	Cost     float64
	Messages int
}

type Store struct{ Dir string }

// ProjectDir returns <app>/projects/<projectID>.
func ProjectDir(projectID string) string {
	return filepath.Join(config.AppDir(), "projects", projectID)
}

func NewStore(projectID string) *Store {
	return &Store{Dir: ProjectDir(projectID)}
}

func (s *Store) sessionsDir() string { return filepath.Join(s.Dir, "sessions") }
func (s *Store) MemoryPath() string  { return filepath.Join(s.Dir, "memory.md") }
func (s *Store) AuditPath() string   { return filepath.Join(s.Dir, "audit.jsonl") }

func NewID() string {
	b := make([]byte, 3)
	_, _ = rand.Read(b)
	return time.Now().Format("20060102-150405") + "-" + hex.EncodeToString(b)
}

func (s *Store) path(id string) string { return filepath.Join(s.sessionsDir(), id+".json") }

// Save writes the session atomically.
func (s *Store) Save(sess *Session) error {
	sess.Updated = time.Now()
	if err := os.MkdirAll(s.sessionsDir(), 0o700); err != nil {
		return err
	}
	b, err := json.Marshal(sess)
	if err != nil {
		return err
	}
	return config.WriteFileAtomic(s.path(sess.ID), b, 0o600)
}

// Load reads a session by id or unique id prefix.
func (s *Store) Load(id string) (*Session, error) {
	full, err := s.resolve(id)
	if err != nil {
		return nil, err
	}
	b, err := os.ReadFile(s.path(full))
	if err != nil {
		return nil, err
	}
	var sess Session
	if err := json.Unmarshal(b, &sess); err != nil {
		return nil, fmt.Errorf("session %s is corrupt: %v", full, err)
	}
	return &sess, nil
}

func (s *Store) resolve(id string) (string, error) {
	if _, err := os.Stat(s.path(id)); err == nil {
		return id, nil
	}
	ids, _ := s.ids()
	var hits []string
	for _, x := range ids {
		if strings.HasPrefix(x, id) || strings.HasSuffix(x, id) {
			hits = append(hits, x)
		}
	}
	switch len(hits) {
	case 0:
		return "", fmt.Errorf("no session %q in this project (tp session list)", id)
	case 1:
		return hits[0], nil
	}
	return "", fmt.Errorf("session id %q is ambiguous (%d matches)", id, len(hits))
}

func (s *Store) ids() ([]string, error) {
	ents, err := os.ReadDir(s.sessionsDir())
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var ids []string
	for _, e := range ents {
		if strings.HasSuffix(e.Name(), ".json") {
			ids = append(ids, strings.TrimSuffix(e.Name(), ".json"))
		}
	}
	return ids, nil
}

// List returns sessions, most recently updated first.
func (s *Store) List() ([]Meta, error) {
	ids, err := s.ids()
	if err != nil {
		return nil, err
	}
	var out []Meta
	for _, id := range ids {
		b, err := os.ReadFile(s.path(id))
		if err != nil {
			continue
		}
		var m struct {
			ID       string            `json:"id"`
			Title    string            `json:"title"`
			Model    string            `json:"model"`
			Status   string            `json:"status"`
			Updated  time.Time         `json:"updated"`
			Turns    int               `json:"turns"`
			Cost     float64           `json:"cost_usd"`
			Messages []json.RawMessage `json:"messages"`
		}
		if json.Unmarshal(b, &m) != nil {
			continue
		}
		out = append(out, Meta{ID: m.ID, Title: m.Title, Model: m.Model, Status: m.Status, Updated: m.Updated, Turns: m.Turns, Cost: m.Cost, Messages: len(m.Messages)})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Updated.After(out[j].Updated) })
	return out, nil
}

// Latest returns the most recently updated session.
func (s *Store) Latest() (*Session, error) {
	l, err := s.List()
	if err != nil {
		return nil, err
	}
	if len(l) == 0 {
		return nil, errors.New("no sessions in this project yet")
	}
	return s.Load(l[0].ID)
}

func (s *Store) Delete(id string) (string, error) {
	full, err := s.resolve(id)
	if err != nil {
		return "", err
	}
	return full, os.Remove(s.path(full))
}

func (s *Store) DeleteAll() (int, error) {
	ids, err := s.ids()
	if err != nil {
		return 0, err
	}
	for _, id := range ids {
		if err := os.Remove(s.path(id)); err != nil {
			return 0, err
		}
	}
	return len(ids), nil
}

// ---- project memory: durable instructions and decisions for this project.

func (s *Store) Memory() string {
	b, _ := os.ReadFile(s.MemoryPath())
	return string(b)
}

func (s *Store) Remember(note string) error {
	if err := os.MkdirAll(s.Dir, 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(s.MemoryPath(), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	_, err = fmt.Fprintf(f, "- %s (%s)\n", strings.TrimSpace(note), time.Now().Format("2006-01-02"))
	return err
}

func (s *Store) ClearMemory() error {
	err := os.Remove(s.MemoryPath())
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// Project describes a project with stored data.
type Project struct {
	ID       string
	Dir      string
	Sessions int
	Root     string
}

// Projects lists every project with stored tp data.
func Projects() ([]Project, error) {
	base := filepath.Join(config.AppDir(), "projects")
	ents, err := os.ReadDir(base)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var out []Project
	for _, e := range ents {
		if !e.IsDir() {
			continue
		}
		st := &Store{Dir: filepath.Join(base, e.Name())}
		p := Project{ID: e.Name(), Dir: st.Dir}
		if l, _ := st.List(); len(l) > 0 {
			p.Sessions = len(l)
			if s, err := st.Load(l[0].ID); err == nil {
				p.Root = s.ProjectRoot
			}
		}
		out = append(out, p)
	}
	return out, nil
}
