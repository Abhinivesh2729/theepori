package workspace

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
)

func sprintf(f string, a ...any) string { return fmt.Sprintf(f, a...) }
func itoa(n int) string                 { return strconv.Itoa(n) }

// Sensitive files are never read, written or listed for the model.
var sensitivePatterns = []string{
	".env", ".env.*", "*.env", "*.pem", "*.key", "*.p12", "*.pfx", "*.jks", "*.keystore",
	"id_rsa*", "id_ed25519*", "id_ecdsa*", "*.ppk", ".npmrc", ".pypirc", ".netrc",
	"credentials", "credentials.json", "*credentials*.json", "service-account*.json",
	"secrets.*", "*.secret", "*.secrets", ".aws/**", ".ssh/**", ".gnupg/**",
	"*.tfstate", "*.tfstate.*", "google-services.json", "GoogleService-Info.plist",
	".git/**",
}

var sensitiveAllow = []string{".env.example", ".env.sample", ".env.template", "*.env.example"}

// Sensitive reports whether a repo-relative path is a likely secret.
func Sensitive(rel string, extra []string) bool {
	rel = filepath.ToSlash(rel)
	base := filepath.Base(rel)
	for _, a := range sensitiveAllow {
		if ok, _ := doublestar.Match(a, base); ok {
			return false
		}
	}
	for _, pat := range append(sensitivePatterns, extra...) {
		if strings.Contains(pat, "/") {
			if ok, _ := doublestar.Match(pat, rel); ok {
				return true
			}
			if ok, _ := doublestar.Match("**/"+pat, rel); ok {
				return true
			}
			continue
		}
		if ok, _ := doublestar.Match(pat, base); ok {
			return true
		}
	}
	return false
}

type Instruction struct {
	Path    string
	Content string
}

// Instructions loads project instruction files (AGENTS.md etc.) from the
// root and from each directory between root and cwd.
func (w *Workspace) Instructions(names []string) []Instruction {
	dirs := []string{w.Root}
	if rel, err := filepath.Rel(w.Root, w.Cwd); err == nil && rel != "." && !strings.HasPrefix(rel, "..") {
		cur := w.Root
		for _, part := range strings.Split(rel, string(filepath.Separator)) {
			cur = filepath.Join(cur, part)
			dirs = append(dirs, cur)
		}
	}
	var out []Instruction
	for _, d := range dirs {
		for _, n := range names {
			p := filepath.Join(d, n)
			b, err := os.ReadFile(p)
			if err != nil || len(strings.TrimSpace(string(b))) == 0 {
				continue
			}
			s := string(b)
			if len(s) > 32*1024 {
				s = s[:32*1024] + "\n…(truncated)"
			}
			rel, _ := filepath.Rel(w.Root, p)
			out = append(out, Instruction{Path: filepath.ToSlash(rel), Content: s})
		}
	}
	return out
}

// Overview summarises the repository layout for the system prompt.
func (w *Workspace) Overview(files []string) string {
	langs := map[string]int{}
	top := map[string]int{}
	for _, f := range files {
		if ext := strings.ToLower(filepath.Ext(f)); ext != "" {
			langs[ext]++
		}
		first, _, found := strings.Cut(f, "/")
		if found {
			top[first+"/"]++
		} else {
			top[first] = 0
		}
	}
	var b strings.Builder
	fmt.Fprintf(&b, "%d files.", len(files))
	if len(langs) > 0 {
		type kv struct {
			k string
			v int
		}
		var l []kv
		for k, v := range langs {
			l = append(l, kv{k, v})
		}
		sort.Slice(l, func(i, j int) bool { return l[i].v > l[j].v || (l[i].v == l[j].v && l[i].k < l[j].k) })
		if len(l) > 8 {
			l = l[:8]
		}
		b.WriteString(" Main file types:")
		for _, x := range l {
			fmt.Fprintf(&b, " %s(%d)", x.k, x.v)
		}
	}
	b.WriteString("\nTop level:\n")
	var names []string
	for k := range top {
		names = append(names, k)
	}
	sort.Strings(names)
	if len(names) > 60 {
		names = append(names[:60], "…")
	}
	for _, n := range names {
		if c := top[n]; c > 0 {
			fmt.Fprintf(&b, "  %s (%d files)\n", n, c)
		} else {
			fmt.Fprintf(&b, "  %s\n", n)
		}
	}
	return b.String()
}

var wordRe = regexp.MustCompile(`[A-Za-z_][A-Za-z0-9_]{2,}`)

var stop = map[string]bool{
	"the": true, "and": true, "for": true, "with": true, "that": true, "this": true, "add": true,
	"fix": true, "make": true, "use": true, "from": true, "into": true, "should": true, "when": true,
	"file": true, "files": true, "code": true, "please": true, "can": true, "you": true, "all": true,
	"new": true, "update": true, "change": true, "test": true, "tests": true, "are": true, "not": true,
}

// Relevant ranks files by overlap between task words and path tokens.
// It is a cheap first hint; the agent still explores with grep/glob.
func Relevant(task string, files []string, limit int) []string {
	words := map[string]bool{}
	for _, w := range wordRe.FindAllString(task, -1) {
		lw := strings.ToLower(w)
		if !stop[lw] {
			words[lw] = true
			for _, part := range splitCamel(w) {
				if len(part) > 2 && !stop[part] {
					words[part] = true
				}
			}
		}
	}
	if len(words) == 0 {
		return nil
	}
	type scored struct {
		f string
		s int
	}
	var res []scored
	for _, f := range files {
		lf := strings.ToLower(f)
		s := 0
		for w := range words {
			if strings.Contains(filepath.Base(lf), w) {
				s += 3
			} else if strings.Contains(lf, w) {
				s++
			}
		}
		if s > 0 {
			res = append(res, scored{f, s})
		}
	}
	sort.Slice(res, func(i, j int) bool { return res[i].s > res[j].s || (res[i].s == res[j].s && res[i].f < res[j].f) })
	var out []string
	for i := 0; i < len(res) && i < limit; i++ {
		out = append(out, res[i].f)
	}
	return out
}

func splitCamel(s string) []string {
	var parts []string
	cur := ""
	for i, r := range s {
		if r == '_' {
			if cur != "" {
				parts = append(parts, strings.ToLower(cur))
			}
			cur = ""
			continue
		}
		if i > 0 && r >= 'A' && r <= 'Z' && cur != "" {
			parts = append(parts, strings.ToLower(cur))
			cur = ""
		}
		cur += string(r)
	}
	if cur != "" {
		parts = append(parts, strings.ToLower(cur))
	}
	return parts
}

// DetectTestCommand guesses the project's test command.
func DetectTestCommand(root string) string {
	exists := func(n string) bool { _, err := os.Stat(filepath.Join(root, n)); return err == nil }
	switch {
	case exists("go.mod"):
		return "go test ./..."
	case exists("Cargo.toml"):
		return "cargo test"
	case exists("package.json"):
		b, _ := os.ReadFile(filepath.Join(root, "package.json"))
		if strings.Contains(string(b), `"test"`) {
			switch {
			case exists("pnpm-lock.yaml"):
				return "pnpm test"
			case exists("yarn.lock"):
				return "yarn test"
			case exists("bun.lockb"), exists("bun.lock"):
				return "bun test"
			}
			return "npm test"
		}
	case exists("pyproject.toml"), exists("pytest.ini"), exists("setup.py"):
		if exists("uv.lock") {
			return "uv run pytest"
		}
		return "pytest"
	case exists("pubspec.yaml"):
		return "flutter test"
	case exists("gradlew"):
		return "./gradlew test"
	case exists("pom.xml"):
		return "mvn test"
	case exists("Makefile"):
		b, _ := os.ReadFile(filepath.Join(root, "Makefile"))
		if regexp.MustCompile(`(?m)^test:`).Match(b) {
			return "make test"
		}
	}
	return ""
}
