package tools

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/bmatcuk/doublestar/v4"

	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

func sensitiveErr(rel string) error {
	return fmt.Errorf("%s looks like a secrets file; tp never reads or writes it. Ask the user to handle it", rel)
}

// ---------------------------------------------------------------- read_file

type ReadFile struct{}

func (ReadFile) Name() string { return "read_file" }
func (ReadFile) Description() string {
	return "Read a text file from the project. Returns numbered lines. Use offset/limit (1-based line numbers) for large files."
}
func (ReadFile) Schema() string {
	return `{"type":"object","properties":{"path":{"type":"string","description":"Path relative to the project root"},"offset":{"type":"integer","description":"First line to read (1-based)"},"limit":{"type":"integer","description":"Maximum lines to read (default 2000)"}},"required":["path"],"additionalProperties":false}`
}

func (t ReadFile) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Path   string `json:"path"`
		Offset int    `json:"offset"`
		Limit  int    `json:"limit"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	abs, rel, err := env.WS.Resolve(in.Path)
	if err != nil {
		return nil, err
	}
	if workspace.Sensitive(rel, nil) {
		return nil, sensitiveErr(rel)
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "read " + rel, Path: rel},
		Run: func(ctx context.Context) Result {
			b, err := os.ReadFile(abs)
			if err != nil {
				return errResult("%v", err)
			}
			if bytes.IndexByte(b[:min(len(b), 8192)], 0) >= 0 {
				return errResult("%s is a binary file", rel)
			}
			lines := strings.Split(string(b), "\n")
			if strings.HasSuffix(string(b), "\n") {
				lines = lines[:len(lines)-1]
			}
			start := max(in.Offset, 1)
			limit := in.Limit
			if limit <= 0 {
				limit = 2000
			}
			if start > len(lines) && len(lines) > 0 {
				return errResult("offset %d is past end of file (%d lines)", start, len(lines))
			}
			end := min(start-1+limit, len(lines))
			var out strings.Builder
			size := 0
			for i := start - 1; i < end; i++ {
				l := lines[i]
				if len(l) > 2000 {
					l = l[:2000] + "…"
				}
				fmt.Fprintf(&out, "%6d\t%s\n", i+1, l)
				size += len(l)
				if size > env.MaxFileBytes {
					end = i + 1
					break
				}
			}
			if end < len(lines) {
				fmt.Fprintf(&out, "… (%d more lines; read with offset=%d)\n", len(lines)-end, end+1)
			}
			if len(lines) == 0 {
				out.WriteString("(empty file)\n")
			}
			return Result{Output: env.Redact.Redact(out.String()), Display: fmt.Sprintf("%s (%s)", rel, plural(end-start+1, "line"))}
		},
	}, nil
}

// ----------------------------------------------------------------- list_dir

type ListDir struct{}

func (ListDir) Name() string { return "list_dir" }
func (ListDir) Description() string {
	return "List files and directories under a path (respects .gitignore). Use to understand repository structure."
}
func (ListDir) Schema() string {
	return `{"type":"object","properties":{"path":{"type":"string","description":"Directory relative to project root (default '.')"},"depth":{"type":"integer","description":"Max depth (default 2)"}},"additionalProperties":false}`
}

func (t ListDir) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Path  string `json:"path"`
		Depth int    `json:"depth"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if in.Path == "" {
		in.Path = "."
	}
	if in.Depth <= 0 {
		in.Depth = 2
	}
	_, rel, err := env.WS.Resolve(in.Path)
	if err != nil {
		return nil, err
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "list " + rel},
		Run: func(ctx context.Context) Result {
			prefix := ""
			if rel != "." {
				prefix = strings.TrimSuffix(rel, "/") + "/"
			}
			entries := map[string]int{}
			for _, f := range env.WS.Files() {
				if !strings.HasPrefix(f, prefix) || workspace.Sensitive(f, nil) {
					continue
				}
				sub := strings.TrimPrefix(f, prefix)
				parts := strings.Split(sub, "/")
				if len(parts) > in.Depth {
					entries[strings.Join(parts[:in.Depth], "/")+"/"]++
				} else {
					entries[sub] = 0
				}
				for d := 1; d < len(parts) && d < in.Depth; d++ {
					if _, ok := entries[strings.Join(parts[:d], "/")+"/"]; !ok {
						entries[strings.Join(parts[:d], "/")+"/"] = -1
					}
				}
			}
			if len(entries) == 0 {
				return errResult("no files under %s", rel)
			}
			keys := make([]string, 0, len(entries))
			for k := range entries {
				keys = append(keys, k)
			}
			sort.Strings(keys)
			var out strings.Builder
			for i, k := range keys {
				if i >= 500 {
					fmt.Fprintf(&out, "… (%d more)\n", len(keys)-500)
					break
				}
				if n := entries[k]; n > 0 {
					fmt.Fprintf(&out, "%s (%d files)\n", k, n)
				} else {
					out.WriteString(k + "\n")
				}
			}
			return Result{Output: out.String(), Display: fmt.Sprintf("%s (%s)", rel, plural(len(keys), "entry"))}
		},
	}, nil
}

// --------------------------------------------------------------------- glob

type Glob struct{}

func (Glob) Name() string { return "glob" }
func (Glob) Description() string {
	return "Find files by glob pattern, e.g. '**/*.go' or 'src/**/test_*.py'. Respects .gitignore."
}
func (Glob) Schema() string {
	return `{"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"],"additionalProperties":false}`
}

func (t Glob) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Pattern string `json:"pattern"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if !doublestar.ValidatePattern(in.Pattern) {
		return nil, fmt.Errorf("invalid glob pattern %q", in.Pattern)
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "glob " + in.Pattern},
		Run: func(ctx context.Context) Result {
			var hits []string
			for _, f := range env.WS.Files() {
				ok, _ := doublestar.Match(in.Pattern, f)
				if !ok && !strings.Contains(in.Pattern, "/") {
					ok, _ = doublestar.Match(in.Pattern, path.Base(f))
				}
				if ok && !workspace.Sensitive(f, nil) {
					hits = append(hits, f)
				}
			}
			if len(hits) == 0 {
				return Result{Output: "No files match.", Display: "0 matches"}
			}
			n := len(hits)
			if n > 300 {
				hits = append(hits[:300], fmt.Sprintf("… (%d more)", n-300))
			}
			return Result{Output: strings.Join(hits, "\n"), Display: plural(n, "file")}
		},
	}, nil
}

// --------------------------------------------------------------------- grep

type Grep struct{}

func (Grep) Name() string { return "grep" }
func (Grep) Description() string {
	return "Search file contents with a regular expression (RE2 syntax). Returns file:line: text. Narrow with path or include glob."
}
func (Grep) Schema() string {
	return `{"type":"object","properties":{"pattern":{"type":"string","description":"RE2 regular expression"},"path":{"type":"string","description":"Directory or file to search (default: project root)"},"include":{"type":"string","description":"Glob to filter files, e.g. '*.ts'"},"ignore_case":{"type":"boolean"},"max_results":{"type":"integer","description":"Default 100"}},"required":["pattern"],"additionalProperties":false}`
}

func (t Grep) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Pattern    string `json:"pattern"`
		Path       string `json:"path"`
		Include    string `json:"include"`
		IgnoreCase bool   `json:"ignore_case"`
		MaxResults int    `json:"max_results"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	pat := in.Pattern
	if in.IgnoreCase {
		pat = "(?i)" + pat
	}
	re, err := regexp.Compile(pat)
	if err != nil {
		return nil, fmt.Errorf("invalid regex: %v", err)
	}
	prefix := ""
	if in.Path != "" && in.Path != "." {
		_, rel, err := env.WS.Resolve(in.Path)
		if err != nil {
			return nil, err
		}
		prefix = rel
	}
	if in.MaxResults <= 0 {
		in.MaxResults = 100
	}
	return &Call{
		Perm: permission.Request{Tool: t.Name(), Risk: permission.RiskRead, Summary: "grep " + in.Pattern},
		Run: func(ctx context.Context) Result {
			var out strings.Builder
			count, files := 0, 0
			for _, f := range env.WS.Files() {
				if ctx.Err() != nil {
					break
				}
				if prefix != "" && f != prefix && !strings.HasPrefix(f, prefix+"/") {
					continue
				}
				if in.Include != "" {
					if ok, _ := doublestar.Match(in.Include, path.Base(f)); !ok {
						if ok2, _ := doublestar.Match(in.Include, f); !ok2 {
							continue
						}
					}
				}
				if workspace.Sensitive(f, nil) {
					continue
				}
				abs := filepath.Join(env.WS.Root, f)
				st, err := os.Stat(abs)
				if err != nil || st.Size() > 2<<20 || st.IsDir() {
					continue
				}
				b, err := os.ReadFile(abs)
				if err != nil || bytes.IndexByte(b[:min(len(b), 8192)], 0) >= 0 {
					continue
				}
				sc := bufio.NewScanner(bytes.NewReader(b))
				sc.Buffer(make([]byte, 0, 64*1024), 4<<20)
				ln, matched := 0, false
				for sc.Scan() {
					ln++
					line := sc.Text()
					if re.MatchString(line) {
						if count < in.MaxResults {
							if len(line) > 240 {
								line = line[:240] + "…"
							}
							fmt.Fprintf(&out, "%s:%d: %s\n", f, ln, strings.TrimSpace(line))
						}
						count++
						matched = true
					}
				}
				if matched {
					files++
				}
			}
			if count == 0 {
				return Result{Output: "No matches.", Display: "0 matches"}
			}
			if count > in.MaxResults {
				fmt.Fprintf(&out, "… (%d more matches; narrow the search)\n", count-in.MaxResults)
			}
			return Result{Output: env.Redact.Redact(out.String()), Display: fmt.Sprintf("%s in %s", plural(count, "match"), plural(files, "file"))}
		},
	}, nil
}

// --------------------------------------------------------------- write_file

type WriteFile struct{}

func (WriteFile) Name() string { return "write_file" }
func (WriteFile) Description() string {
	return "Create a new file or fully overwrite an existing one. Prefer edit_file for changes to existing files."
}
func (WriteFile) Schema() string {
	return `{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string","description":"Complete file content"}},"required":["path","content"],"additionalProperties":false}`
}

func (t WriteFile) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Path    *string `json:"path"`
		Content *string `json:"content"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if in.Path == nil || in.Content == nil {
		return nil, fmt.Errorf("path and content are required (input may have been truncated)")
	}
	abs, rel, err := env.WS.Resolve(*in.Path)
	if err != nil {
		return nil, err
	}
	if workspace.Sensitive(rel, nil) {
		return nil, sensitiveErr(rel)
	}
	old, existed := readMaybe(abs)
	diff := UnifiedDiff(rel, old, *in.Content, existed, true)
	add, del := DiffStat(diff)
	verb := "create"
	if existed {
		verb = "overwrite"
	}
	return &Call{
		Perm: permission.Request{
			Tool: t.Name(), Risk: permission.RiskWrite, Path: rel, Key: "write",
			Summary: fmt.Sprintf("%s %s (+%d -%d)", verb, rel, add, del), Detail: diff,
		},
		Run: func(ctx context.Context) Result {
			env.Tracker.BeforeWrite(rel)
			if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
				return errResult("%v", err)
			}
			mode := os.FileMode(0o644)
			if st, err := os.Stat(abs); err == nil {
				mode = st.Mode().Perm()
			}
			if err := os.WriteFile(abs, []byte(*in.Content), mode); err != nil {
				return errResult("%v", err)
			}
			return Result{Output: fmt.Sprintf("Wrote %s (+%d -%d lines).", rel, add, del), Display: fmt.Sprintf("%s (+%d -%d)", rel, add, del)}
		},
	}, nil
}

func readMaybe(abs string) (string, bool) {
	b, err := os.ReadFile(abs)
	if err != nil {
		return "", false
	}
	return string(b), true
}

// ---------------------------------------------------------------- edit_file

type EditFile struct{}

func (EditFile) Name() string { return "edit_file" }
func (EditFile) Description() string {
	return "Replace exact text in a file. old_string must match the file exactly (including whitespace) and be unique unless replace_all is true. Read the file first."
}
func (EditFile) Schema() string {
	return `{"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string","description":"Exact text to replace"},"new_string":{"type":"string","description":"Replacement text"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"],"additionalProperties":false}`
}

func (t EditFile) Prepare(env *Env, input json.RawMessage) (*Call, error) {
	var in struct {
		Path       string  `json:"path"`
		OldString  *string `json:"old_string"`
		NewString  *string `json:"new_string"`
		ReplaceAll bool    `json:"replace_all"`
	}
	if err := decode(input, &in); err != nil {
		return nil, err
	}
	if in.OldString == nil || in.NewString == nil {
		return nil, fmt.Errorf("old_string and new_string are required (input may have been truncated)")
	}
	abs, rel, err := env.WS.Resolve(in.Path)
	if err != nil {
		return nil, err
	}
	if workspace.Sensitive(rel, nil) {
		return nil, sensitiveErr(rel)
	}
	old, existed := readMaybe(abs)
	if !existed {
		return nil, fmt.Errorf("%s does not exist; use write_file to create it", rel)
	}
	if *in.OldString == "" {
		return nil, fmt.Errorf("old_string is empty; use write_file to replace a whole file")
	}
	if *in.OldString == *in.NewString {
		return nil, fmt.Errorf("old_string and new_string are identical")
	}
	n := strings.Count(old, *in.OldString)
	switch {
	case n == 0:
		hint := ""
		if strings.Contains(normalizeWS(old), normalizeWS(*in.OldString)) {
			hint = " (it matches if whitespace is ignored — copy indentation exactly)"
		}
		return nil, fmt.Errorf("old_string not found in %s%s; read the file again and use its exact text", rel, hint)
	case n > 1 && !in.ReplaceAll:
		return nil, fmt.Errorf("old_string occurs %d times in %s; include more surrounding context or set replace_all", n, rel)
	}
	updated := strings.Replace(old, *in.OldString, *in.NewString, map[bool]int{true: -1, false: 1}[in.ReplaceAll])
	diff := UnifiedDiff(rel, old, updated, true, true)
	add, del := DiffStat(diff)
	return &Call{
		Perm: permission.Request{
			Tool: t.Name(), Risk: permission.RiskWrite, Path: rel, Key: "write",
			Summary: fmt.Sprintf("edit %s (+%d -%d)", rel, add, del), Detail: diff,
		},
		Run: func(ctx context.Context) Result {
			// Re-check: the file may have changed since preparation.
			if cur, _ := readMaybe(abs); cur != old {
				return errResult("%s changed since the edit was prepared; read it again", rel)
			}
			env.Tracker.BeforeWrite(rel)
			st, _ := os.Stat(abs)
			if err := os.WriteFile(abs, []byte(updated), st.Mode().Perm()); err != nil {
				return errResult("%v", err)
			}
			msg := fmt.Sprintf("Edited %s (+%d -%d lines).", rel, add, del)
			if in.ReplaceAll && n > 1 {
				msg = fmt.Sprintf("Edited %s: replaced %d occurrences (+%d -%d lines).", rel, n, add, del)
			}
			return Result{Output: msg, Display: fmt.Sprintf("%s (+%d -%d)", rel, add, del)}
		},
	}, nil
}

var wsRe = regexp.MustCompile(`\s+`)

func normalizeWS(s string) string { return wsRe.ReplaceAllString(strings.TrimSpace(s), " ") }
