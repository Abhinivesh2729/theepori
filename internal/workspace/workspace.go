// Package workspace inspects the repository: root/git detection, file
// listing with ignore rules, sensitive-file protection, project
// instructions and lightweight relevance ranking.
package workspace

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
)

type Workspace struct {
	Root   string
	Cwd    string
	IsGit  bool
	Ignore []string // extra glob patterns from config
}

// Detect finds the repository root (git top-level, else cwd).
func Detect(cwd string, ignore []string) *Workspace {
	abs, _ := filepath.Abs(cwd)
	w := &Workspace{Root: abs, Cwd: abs, Ignore: ignore}
	if out, err := Git(abs, "rev-parse", "--show-toplevel"); err == nil {
		w.Root = strings.TrimSpace(out)
		w.IsGit = true
	}
	return w
}

// ProjectID is a stable, filesystem-safe id for per-project storage.
func (w *Workspace) ProjectID() string {
	sum := sha256.Sum256([]byte(w.Root))
	name := regexp.MustCompile(`[^A-Za-z0-9._-]+`).ReplaceAllString(filepath.Base(w.Root), "-")
	return name + "-" + hex.EncodeToString(sum[:])[:10]
}

// Git runs a git command in dir and returns stdout.
func Git(dir string, args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	if err := cmd.Run(); err != nil {
		if errb.Len() > 0 {
			return out.String(), &GitError{Msg: strings.TrimSpace(errb.String())}
		}
		return out.String(), err
	}
	return out.String(), nil
}

type GitError struct{ Msg string }

func (e *GitError) Error() string { return "git: " + e.Msg }

// Branch returns the current branch name ("" outside git).
func (w *Workspace) Branch() string {
	if !w.IsGit {
		return ""
	}
	// symbolic-ref also works before the first commit.
	if out, err := Git(w.Root, "symbolic-ref", "--short", "-q", "HEAD"); err == nil && strings.TrimSpace(out) != "" {
		return strings.TrimSpace(out)
	}
	out, err := Git(w.Root, "rev-parse", "--short", "HEAD")
	if err != nil {
		return ""
	}
	return "detached@" + strings.TrimSpace(out)
}

// Status is a short git status summary.
func (w *Workspace) Status() string {
	if !w.IsGit {
		return "(not a git repository)"
	}
	out, err := Git(w.Root, "status", "--short", "--branch")
	if err != nil {
		return "(git status failed)"
	}
	lines := strings.Split(strings.TrimRight(out, "\n"), "\n")
	if len(lines) > 30 {
		lines = append(lines[:30], "… ("+itoa(len(lines)-30)+" more)")
	}
	return strings.Join(lines, "\n")
}

var defaultIgnoreDirs = map[string]bool{
	".git": true, "node_modules": true, "vendor": true, "dist": true, "build": true,
	"target": true, ".venv": true, "venv": true, "__pycache__": true, ".next": true,
	".idea": true, ".gradle": true, ".dart_tool": true, "Pods": true, ".theepori": false,
}

// Files lists repo files relative to Root, honouring .gitignore (via git)
// and config ignore patterns.
func (w *Workspace) Files() []string {
	var files []string
	if w.IsGit {
		out, err := Git(w.Root, "ls-files", "--cached", "--others", "--exclude-standard", "-z")
		if err == nil {
			for _, f := range strings.Split(out, "\x00") {
				if f != "" {
					files = append(files, f)
				}
			}
		}
	}
	if files == nil {
		_ = filepath.WalkDir(w.Root, func(p string, d fs.DirEntry, err error) error {
			if err != nil {
				return nil
			}
			if d.IsDir() {
				if p != w.Root && (defaultIgnoreDirs[d.Name()] || strings.HasPrefix(d.Name(), ".")) {
					return filepath.SkipDir
				}
				return nil
			}
			rel, _ := filepath.Rel(w.Root, p)
			files = append(files, filepath.ToSlash(rel))
			if len(files) > 50_000 {
				return fs.SkipAll
			}
			return nil
		})
	}
	out := files[:0]
	for _, f := range files {
		if !w.Ignored(f) {
			out = append(out, f)
		}
	}
	sort.Strings(out)
	return out
}

// Ignored reports whether a repo-relative path matches config ignore patterns.
func (w *Workspace) Ignored(rel string) bool {
	rel = filepath.ToSlash(rel)
	for _, pat := range w.Ignore {
		if ok, _ := doublestar.Match(pat, rel); ok {
			return true
		}
		if ok, _ := doublestar.Match(pat, filepath.Base(rel)); ok && !strings.Contains(pat, "/") {
			return true
		}
	}
	return false
}

// Resolve turns a model-supplied path into an absolute path inside Root.
// It rejects escapes via .. or symlinks.
func (w *Workspace) Resolve(p string) (abs, rel string, err error) {
	if p == "" {
		return "", "", errf("path is required")
	}
	if !filepath.IsAbs(p) {
		p = filepath.Join(w.Root, p)
	}
	abs = filepath.Clean(p)
	rel, err = filepath.Rel(w.Root, abs)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", "", errf("path %q is outside the project root %s", p, w.Root)
	}
	// Follow symlinks of the existing prefix.
	if real, err := filepath.EvalSymlinks(existingPrefix(abs)); err == nil {
		rootReal, _ := filepath.EvalSymlinks(w.Root)
		if r, err := filepath.Rel(rootReal, real); err != nil || strings.HasPrefix(r, "..") {
			return "", "", errf("path %q resolves outside the project root", p)
		}
	}
	return abs, filepath.ToSlash(rel), nil
}

func existingPrefix(p string) string {
	for {
		if _, err := os.Lstat(p); err == nil {
			return p
		}
		parent := filepath.Dir(p)
		if parent == p {
			return p
		}
		p = parent
	}
}

type pathError string

func (e pathError) Error() string { return string(e) }

func errf(format string, a ...any) error {
	return pathError(sprintf(format, a...))
}
