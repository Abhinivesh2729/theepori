// Package ui renders the agent in a terminal: streaming text, tool activity,
// approval prompts with diffs, and turn summaries. It is line-based (no
// full-screen TUI), so it also works when piped.
package ui

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/agent"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/tools"
)

var ErrInterrupted = errors.New("interrupted")

type Console struct {
	Out          io.Writer
	Color        bool
	ShowThinking bool
	Interactive  bool // a human can answer prompts

	lines     chan string
	interrupt chan struct{}
	mu        sync.Mutex
	kind      string // last output kind: text | thinking | tool | ""
	atBOL     bool
}

// IsTerminal reports whether f is a character device.
func IsTerminal(f *os.File) bool {
	st, err := f.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}

func NewConsole(interactive bool) *Console {
	color := IsTerminal(os.Stdout) && os.Getenv("NO_COLOR") == "" && os.Getenv("TERM") != "dumb"
	return &Console{Out: os.Stdout, Color: color, ShowThinking: true, Interactive: interactive,
		interrupt: make(chan struct{}, 1), atBOL: true}
}

// StartInput begins reading stdin lines in the background.
func (c *Console) StartInput(r io.Reader) {
	c.lines = make(chan string)
	go func() {
		defer close(c.lines)
		br := bufio.NewReader(r)
		for {
			s, err := br.ReadString('\n')
			if len(s) > 0 || err == nil {
				c.lines <- strings.TrimRight(s, "\r\n")
			}
			if err != nil {
				return
			}
		}
	}()
}

// Interrupt wakes a pending ReadLine with ErrInterrupted.
func (c *Console) Interrupt() {
	select {
	case c.interrupt <- struct{}{}:
	default:
	}
}

// ReadLine prints prompt and waits for a line, Ctrl-C (ErrInterrupted),
// EOF (io.EOF) or ctx cancellation.
func (c *Console) ReadLine(ctx context.Context, prompt string) (string, error) {
	if c.lines == nil {
		return "", io.EOF
	}
	// Drop a stale interrupt from before the prompt was shown.
	select {
	case <-c.interrupt:
	default:
	}
	c.printf("%s", prompt)
	select {
	case l, ok := <-c.lines:
		if !ok {
			return "", io.EOF
		}
		c.atBOL = true
		return l, nil
	case <-c.interrupt:
		c.printf("\n")
		return "", ErrInterrupted
	case <-ctx.Done():
		c.printf("\n")
		return "", ctx.Err()
	}
}

// ReadInput reads a possibly multi-line message: end a line with "\" to
// continue, or wrap a block in """ lines.
func (c *Console) ReadInput(ctx context.Context, prompt string) (string, error) {
	first, err := c.ReadLine(ctx, prompt)
	if err != nil {
		return "", err
	}
	var b strings.Builder
	switch {
	case strings.TrimSpace(first) == `"""`:
		for {
			l, err := c.ReadLine(ctx, c.dim("… "))
			if err != nil {
				return "", err
			}
			if strings.TrimSpace(l) == `"""` {
				return b.String(), nil
			}
			b.WriteString(l + "\n")
		}
	case strings.HasSuffix(first, `\`):
		b.WriteString(strings.TrimSuffix(first, `\`) + "\n")
		for {
			l, err := c.ReadLine(ctx, c.dim("… "))
			if err != nil {
				return "", err
			}
			if !strings.HasSuffix(l, `\`) {
				b.WriteString(l)
				return b.String(), nil
			}
			b.WriteString(strings.TrimSuffix(l, `\`) + "\n")
		}
	}
	return first, nil
}

func (c *Console) printf(format string, a ...any) {
	c.mu.Lock()
	defer c.mu.Unlock()
	s := fmt.Sprintf(format, a...)
	if s == "" {
		return
	}
	fmt.Fprint(c.Out, s)
	c.atBOL = strings.HasSuffix(s, "\n")
}

func (c *Console) newline() {
	if !c.atBOL {
		c.printf("\n")
	}
}

// switchKind separates different kinds of output with a blank line.
func (c *Console) switchKind(k string) {
	if c.kind == k {
		return
	}
	c.newline()
	if c.kind != "" && (k == "text" || c.kind == "text") {
		c.printf("\n")
	}
	c.kind = k
}

// ---- agent.UI

func (c *Console) Text(delta string) {
	c.switchKind("text")
	c.printf("%s", delta)
}

func (c *Console) Thinking(delta string) {
	if !c.ShowThinking {
		return
	}
	if c.kind != "thinking" {
		c.switchKind("thinking")
		c.printf("%s", c.dim("✻ "))
	}
	c.printf("%s", c.dim(delta))
}

func (c *Console) ToolStreaming(name string) {}

func (c *Console) ToolCall(req permission.Request) {
	c.switchKind("tool")
	marker := c.cyan("●")
	switch req.Risk {
	case permission.RiskWrite:
		marker = c.yellow("●")
	case permission.RiskExec:
		marker = c.magenta("●")
	case permission.RiskDestructive:
		marker = c.red("●")
	}
	c.printf("%s %s %s\n", marker, c.bold(req.Tool), oneLine(req.Summary, 120))
}

func (c *Console) ToolResult(tool string, res tools.Result) {
	c.switchKind("tool")
	d := oneLine(res.Display, 140)
	if d == "" {
		d = "done"
	}
	if res.IsError {
		c.printf("  %s %s\n", c.dim("⎿"), c.red(d))
		return
	}
	c.printf("  %s %s\n", c.dim("⎿"), c.dim(d))
}

func (c *Console) Plan(steps []tools.PlanStep, note string) {
	c.switchKind("tool")
	if note != "" {
		c.printf("  %s\n", c.dim(oneLine(note, 160)))
	}
	for _, s := range steps {
		switch s.Status {
		case "completed":
			c.printf("  %s %s\n", c.green("✔"), c.dim(s.Step))
		case "in_progress":
			c.printf("  %s %s\n", c.yellow("▸"), c.bold(s.Step))
		default:
			c.printf("  %s %s\n", c.dim("□"), s.Step)
		}
	}
}

func (c *Console) Notice(msg string) {
	c.switchKind("tool")
	c.printf("%s\n", c.yellow("! "+msg))
}

func (c *Console) Info(msg string) {
	c.newline()
	c.printf("%s\n", c.dim(msg))
}

func (c *Console) Error(err error) {
	c.newline()
	c.printf("%s\n", c.red("✗ "+err.Error()))
}

func (c *Console) TurnEnd(s agent.TurnSummary) {
	c.newline()
	c.kind = ""
	if len(s.Changed) > 0 {
		c.printf("\n%s\n", c.bold("Changes"))
		lines := strings.Count(s.Diff, "\n")
		if lines <= 150 {
			c.printf("%s", c.Diff(s.Diff, 150))
		} else {
			for _, f := range s.Changed {
				c.printf("  %s\n", f)
			}
			c.printf("%s\n", c.dim(fmt.Sprintf("  (%d diff lines — /diff to view)", lines)))
		}
	}
	if len(s.Tests) > 0 {
		c.printf("\n%s\n", c.bold("Tests"))
		for _, t := range s.Tests {
			mark := c.green("✔ passed")
			if !t.Passed {
				mark = c.red("✗ failed")
			}
			c.printf("  %s  %s · %s · %s\n", mark, t.Command, t.Summary, t.Duration.Round(100*time.Millisecond))
		}
	}
	if s.Stopped != "" {
		c.printf("\n%s\n", c.yellow("! "+s.Stopped))
	}
	u := s.Usage
	if u.InputTokens+u.OutputTokens+u.CacheReadTokens > 0 {
		cache := ""
		if u.CacheReadTokens > 0 {
			cache = fmt.Sprintf(" (+%s cached)", kTok(u.CacheReadTokens))
		}
		cost := ""
		if s.Cost > 0 {
			cost = fmt.Sprintf(" · ~$%.4f", s.Cost)
		}
		c.printf("\n%s\n", c.dim(fmt.Sprintf("%s in%s · %s out%s · %d step(s)", kTok(u.InputTokens+u.CacheWriteTokens), cache, kTok(u.OutputTokens), cost, s.Steps)))
	}
}

// Diff colours a unified diff, capped to max lines.
func (c *Console) Diff(diff string, max int) string {
	var b strings.Builder
	lines := strings.Split(strings.TrimRight(diff, "\n"), "\n")
	for i, l := range lines {
		if max > 0 && i >= max {
			b.WriteString(c.dim(fmt.Sprintf("  … %d more lines\n", len(lines)-max)))
			break
		}
		switch {
		case strings.HasPrefix(l, "+++"), strings.HasPrefix(l, "---"):
			b.WriteString(c.bold(l))
		case strings.HasPrefix(l, "+"):
			b.WriteString(c.green(l))
		case strings.HasPrefix(l, "-"):
			b.WriteString(c.red(l))
		case strings.HasPrefix(l, "@@"):
			b.WriteString(c.cyan(l))
		default:
			b.WriteString(c.dim(l))
		}
		b.WriteString("\n")
	}
	return b.String()
}

// ---- permission.Approver

func (c *Console) Approve(ctx context.Context, req permission.Request) (permission.Answer, error) {
	if !c.Interactive {
		return permission.Deny{}.Approve(ctx, req)
	}
	c.newline()
	bar := c.yellow("│")
	head := fmt.Sprintf("%s %s · %s", c.yellow("╭─"), c.bold(req.Tool), req.Risk)
	if req.Risk == permission.RiskDestructive {
		bar = c.red("│")
		head = fmt.Sprintf("%s %s · %s", c.red("╭─"), c.bold(req.Tool), c.red("DESTRUCTIVE"))
	}
	c.printf("%s\n", head)
	if req.Reason != "" {
		c.printf("%s %s\n", bar, c.red("⚠ "+req.Reason))
	}
	if req.Detail != "" && req.Detail != req.Command {
		for _, l := range strings.Split(strings.TrimRight(c.Diff(req.Detail, 80), "\n"), "\n") {
			c.printf("%s %s\n", bar, l)
		}
	} else {
		for _, l := range strings.Split(req.Summary, "\n") {
			c.printf("%s %s\n", bar, l)
		}
	}
	for {
		var prompt string
		if req.Risk == permission.RiskDestructive {
			prompt = fmt.Sprintf("%s Type %s to allow, anything else denies (feedback is passed to the agent): ", c.red("╰─"), c.bold("yes"))
		} else {
			prompt = fmt.Sprintf("%s Allow? %s yes · %s always this session · %s no — or type feedback: ", c.yellow("╰─"), c.bold("[y]"), c.bold("[a]"), c.bold("[n]"))
		}
		ans, err := c.ReadLine(ctx, prompt)
		if err != nil {
			return permission.Answer{Feedback: "interrupted"}, nil
		}
		a := strings.TrimSpace(ans)
		la := strings.ToLower(a)
		if req.Risk == permission.RiskDestructive {
			if la == "yes" {
				return permission.Answer{Allow: true}, nil
			}
			return permission.Answer{Feedback: feedback(a)}, nil
		}
		switch la {
		case "", "y", "yes":
			return permission.Answer{Allow: true}, nil
		case "a", "always":
			return permission.Answer{Allow: true, Always: true}, nil
		case "n", "no":
			return permission.Answer{}, nil
		}
		return permission.Answer{Feedback: a}, nil
	}
}

func feedback(a string) string {
	switch strings.ToLower(a) {
	case "", "n", "no":
		return ""
	}
	return a
}

// Ask implements the ask_user tool.
func (c *Console) Ask(ctx context.Context, q string) (string, error) {
	c.newline()
	c.printf("%s %s\n", c.cyan("?"), c.bold(q))
	return c.ReadInput(ctx, c.cyan("› "))
}

// ---- styling

func (c *Console) wrap(code, s string) string {
	if !c.Color || s == "" {
		return s
	}
	return "\x1b[" + code + "m" + s + "\x1b[0m"
}

func (c *Console) dim(s string) string     { return c.wrap("2", s) }
func (c *Console) bold(s string) string    { return c.wrap("1", s) }
func (c *Console) red(s string) string     { return c.wrap("31", s) }
func (c *Console) green(s string) string   { return c.wrap("32", s) }
func (c *Console) yellow(s string) string  { return c.wrap("33", s) }
func (c *Console) magenta(s string) string { return c.wrap("35", s) }
func (c *Console) cyan(s string) string    { return c.wrap("36", s) }

// Exported styles for the CLI.
func (c *Console) Dim(s string) string   { return c.dim(s) }
func (c *Console) Bold(s string) string  { return c.bold(s) }
func (c *Console) Green(s string) string { return c.green(s) }
func (c *Console) Red(s string) string   { return c.red(s) }
func (c *Console) Cyan(s string) string  { return c.cyan(s) }
func (c *Console) Yellow(s string) string {
	return c.yellow(s)
}

func (c *Console) Printf(format string, a ...any) { c.printf(format, a...) }

func oneLine(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > n {
		return s[:n-1] + "…"
	}
	return s
}

func kTok(n int) string {
	switch {
	case n >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(n)/1e6)
	case n >= 1000:
		return fmt.Sprintf("%.1fk", float64(n)/1e3)
	}
	return fmt.Sprintf("%d", n)
}
