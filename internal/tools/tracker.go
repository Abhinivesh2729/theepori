package tools

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/pmezard/go-difflib/difflib"
)

// Snapshot is a file's content before tp first touched it.
type Snapshot struct {
	Existed bool   `json:"existed"`
	Content string `json:"content,omitempty"`
	Skipped bool   `json:"skipped,omitempty"` // too large/binary to keep
}

type TestRecord struct {
	Command  string        `json:"command"`
	ExitCode int           `json:"exit_code"`
	Passed   bool          `json:"passed"`
	Summary  string        `json:"summary"`
	Duration time.Duration `json:"duration"`
	At       time.Time     `json:"at"`
}

type CommandRecord struct {
	Command  string        `json:"command"`
	ExitCode int           `json:"exit_code"`
	Duration time.Duration `json:"duration"`
	At       time.Time     `json:"at"`
}

// TrackerState is the persisted form (stored in the session).
type TrackerState struct {
	Root      string                `json:"root"`
	Originals map[string]Snapshot   `json:"originals"` // session baseline
	Turns     []map[string]Snapshot `json:"turns"`     // per-turn checkpoints for /undo
	Tests     []TestRecord          `json:"tests,omitempty"`
	Commands  []CommandRecord       `json:"commands,omitempty"`
}

// Tracker records modified files, checkpoints and test/command results.
type Tracker struct {
	mu sync.Mutex
	s  TrackerState
}

const maxSnapshot = 2 << 20

func NewTracker(root string, st *TrackerState) *Tracker {
	t := &Tracker{}
	if st != nil {
		t.s = *st
	}
	t.s.Root = root
	if t.s.Originals == nil {
		t.s.Originals = map[string]Snapshot{}
	}
	return t
}

func (t *Tracker) State() TrackerState {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.s
}

// StartTurn opens a new checkpoint.
func (t *Tracker) StartTurn() {
	t.mu.Lock()
	t.s.Turns = append(t.s.Turns, map[string]Snapshot{})
	t.mu.Unlock()
}

func snapshot(abs string) Snapshot {
	b, err := os.ReadFile(abs)
	if err != nil {
		return Snapshot{Existed: false}
	}
	if len(b) > maxSnapshot || bytes.IndexByte(b, 0) >= 0 {
		return Snapshot{Existed: true, Skipped: true}
	}
	return Snapshot{Existed: true, Content: string(b)}
}

// BeforeWrite must be called before modifying rel.
func (t *Tracker) BeforeWrite(rel string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	abs := filepath.Join(t.s.Root, rel)
	if _, ok := t.s.Originals[rel]; !ok {
		t.s.Originals[rel] = snapshot(abs)
	}
	if len(t.s.Turns) == 0 {
		t.s.Turns = append(t.s.Turns, map[string]Snapshot{})
	}
	cur := t.s.Turns[len(t.s.Turns)-1]
	if _, ok := cur[rel]; !ok {
		cur[rel] = snapshot(abs)
	}
}

func (t *Tracker) AddTest(r TestRecord) {
	t.mu.Lock()
	t.s.Tests = append(t.s.Tests, r)
	t.mu.Unlock()
}

func (t *Tracker) AddCommand(r CommandRecord) {
	t.mu.Lock()
	t.s.Commands = append(t.s.Commands, r)
	if len(t.s.Commands) > 200 {
		t.s.Commands = t.s.Commands[len(t.s.Commands)-200:]
	}
	t.mu.Unlock()
}

// Changed lists files modified this session whose content differs from baseline.
func (t *Tracker) Changed() []string {
	t.mu.Lock()
	defer t.mu.Unlock()
	var out []string
	for rel, snap := range t.s.Originals {
		cur := snapshot(filepath.Join(t.s.Root, rel))
		if cur.Existed != snap.Existed || cur.Content != snap.Content || snap.Skipped {
			out = append(out, rel)
		}
	}
	sort.Strings(out)
	return out
}

// TurnChanged lists files touched in the latest turn.
func (t *Tracker) TurnChanged() []string {
	t.mu.Lock()
	defer t.mu.Unlock()
	if len(t.s.Turns) == 0 {
		return nil
	}
	var out []string
	for rel := range t.s.Turns[len(t.s.Turns)-1] {
		out = append(out, rel)
	}
	sort.Strings(out)
	return out
}

// TestsSince returns test records from time `since`.
func (t *Tracker) TestsSince(since time.Time) []TestRecord {
	t.mu.Lock()
	defer t.mu.Unlock()
	var out []TestRecord
	for _, r := range t.s.Tests {
		if !r.At.Before(since) {
			out = append(out, r)
		}
	}
	return out
}

// Diff renders a unified diff of session changes (or only the given files).
func (t *Tracker) Diff(files []string) string {
	if files == nil {
		files = t.Changed()
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	var b strings.Builder
	for _, rel := range files {
		snap, ok := t.s.Originals[rel]
		if !ok {
			continue
		}
		cur := snapshot(filepath.Join(t.s.Root, rel))
		if snap.Skipped || cur.Skipped {
			fmt.Fprintf(&b, "Binary or large file changed: %s\n", rel)
			continue
		}
		b.WriteString(UnifiedDiff(rel, snap.Content, cur.Content, snap.Existed, cur.Existed))
	}
	return b.String()
}

// Undo restores files to their state before the latest turn that changed files.
func (t *Tracker) Undo() ([]string, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for len(t.s.Turns) > 0 && len(t.s.Turns[len(t.s.Turns)-1]) == 0 {
		t.s.Turns = t.s.Turns[:len(t.s.Turns)-1]
	}
	if len(t.s.Turns) == 0 {
		return nil, errors.New("nothing to undo")
	}
	last := t.s.Turns[len(t.s.Turns)-1]
	var restored []string
	for rel, snap := range last {
		abs := filepath.Join(t.s.Root, rel)
		switch {
		case snap.Skipped:
			continue
		case !snap.Existed:
			if err := os.Remove(abs); err != nil && !os.IsNotExist(err) {
				return restored, err
			}
		default:
			if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
				return restored, err
			}
			if err := os.WriteFile(abs, []byte(snap.Content), 0o644); err != nil {
				return restored, err
			}
		}
		restored = append(restored, rel)
	}
	t.s.Turns = t.s.Turns[:len(t.s.Turns)-1]
	sort.Strings(restored)
	return restored, nil
}

// UnifiedDiff renders old→new for one file.
func UnifiedDiff(rel, old, new string, oldExists, newExists bool) string {
	if old == new && oldExists == newExists {
		return ""
	}
	from, to := "a/"+rel, "b/"+rel
	if !oldExists {
		from = "/dev/null"
	}
	if !newExists {
		to = "/dev/null"
	}
	d := difflib.UnifiedDiff{
		A:        splitLines(old),
		B:        splitLines(new),
		FromFile: from,
		ToFile:   to,
		Context:  3,
	}
	s, _ := difflib.GetUnifiedDiffString(d)
	return s
}

// splitLines splits keeping newlines; difflib.SplitLines would add a
// phantom empty line after a trailing newline.
func splitLines(s string) []string {
	if s == "" {
		return nil
	}
	lines := strings.SplitAfter(s, "\n")
	if lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	} else {
		lines[len(lines)-1] += "\n"
	}
	return lines
}

// DiffStat counts added/removed lines in a unified diff.
func DiffStat(diff string) (add, del int) {
	for _, l := range strings.Split(diff, "\n") {
		switch {
		case strings.HasPrefix(l, "+++"), strings.HasPrefix(l, "---"):
		case strings.HasPrefix(l, "+"):
			add++
		case strings.HasPrefix(l, "-"):
			del++
		}
	}
	return
}
