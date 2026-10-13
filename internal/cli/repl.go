package cli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/agent"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/session"
	"github.com/Abhinivesh2729/theepori/internal/tools"
	"github.com/Abhinivesh2729/theepori/internal/ui"
)

type repl struct {
	a   *agent.Agent
	con *ui.Console
	sig *signals
}

func runInteractive(ctx context.Context, model, mode, resume string) error {
	con := ui.NewConsole(true)
	con.StartInput(os.Stdin)
	con.Printf("\n%s\n", con.Logo(Version))
	a, err := newAgent(con, model, mode, resume)
	if err != nil {
		if a == nil {
			return err
		}
		// Start anyway so the user can fix the model with /model.
		con.Error(err)
	}
	r := &repl{a: a, con: con, sig: newSignals(con)}
	defer r.sig.stop()
	r.banner()
	return r.loop(ctx)
}

func (r *repl) banner() {
	c, a := r.con, r.a
	home, _ := os.UserHomeDir()
	root := a.WS.Root
	if home != "" && strings.HasPrefix(root, home) {
		root = "~" + strings.TrimPrefix(root, home)
	}
	branch := ""
	if b := a.WS.Branch(); b != "" {
		branch = " (" + b + ")"
	}
	c.Printf("%s%s · %s · mode %s\n", root, branch, c.Cyan(orNone(a.Session.Model)), c.Yellow(a.Perms.Mode()))
	for _, w := range a.Cfg.Warnings {
		c.Printf("%s\n", c.Yellow("! "+w))
	}
	if len(a.Session.Messages) > 0 {
		c.Printf("%s\n", c.Dim(fmt.Sprintf("resumed session %s — %q (%d messages, ~$%.4f)", a.Session.ID, a.Session.Title, len(a.Session.Messages), a.Session.Cost)))
		if last := lastAssistantText(a.Session.Messages); last != "" {
			c.Printf("%s\n", c.Dim("last reply: "+clipLine(last, 300)))
		}
		if a.Session.Status == session.StatusInterrupted {
			c.Printf("%s\n", c.Yellow("this session was interrupted — type \"continue\" to resume the task"))
		}
	}
	c.Printf("%s\n\n", c.Dim("/help for commands · Ctrl-C interrupts · Ctrl-D exits"))
}

func (r *repl) loop(ctx context.Context) error {
	var lastInt time.Time
	for {
		line, err := r.con.ReadInput(ctx, r.con.Bold("› "))
		switch {
		case errors.Is(err, io.EOF):
			r.con.Printf("\n")
			return nil
		case errors.Is(err, ui.ErrInterrupted):
			if time.Since(lastInt) < 2*time.Second {
				return nil
			}
			lastInt = time.Now()
			r.con.Info("(press Ctrl-C again or Ctrl-D to exit)")
			continue
		case err != nil:
			return err
		}
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "/") {
			if quit := r.slash(ctx, line); quit {
				return nil
			}
			continue
		}
		r.turn(ctx, line)
	}
}

func (r *repl) turn(ctx context.Context, input string) {
	if r.a.Primary().Provider == nil {
		r.con.Error(errors.New("no working model — pick one with /model <provider/model>"))
		return
	}
	tctx, done := r.sig.turn(ctx)
	defer done()
	if err := r.a.Run(tctx, input); err != nil {
		r.con.Error(err)
	}
	r.con.Printf("\n")
}

var slashHelp = [][2]string{
	{"/help", "show this help"},
	{"/model [provider/model]", "show or switch the model"},
	{"/models", "list configured models"},
	{"/mode [plan|default|accept-edits|auto]", "show or change the permission mode"},
	{"/plan <task>", "investigate read-only and propose a plan"},
	{"/review [base]", "review current changes (or against a base ref)"},
	{"/diff", "show all changes made in this session"},
	{"/undo", "revert files changed in the last turn"},
	{"/compact", "summarise the conversation to free context"},
	{"/status", "session, model, usage and cost"},
	{"/remember <note>", "save a decision/preference to project memory"},
	{"/memory", "show project memory"},
	{"/sessions", "list sessions in this project"},
	{"/resume <id>", "switch to another session"},
	{"/new", "start a new session"},
	{"/tools", "list tools"},
	{"/exit", "quit"},
}

func (r *repl) slash(ctx context.Context, line string) (quit bool) {
	c, a := r.con, r.a
	cmd, arg, _ := strings.Cut(line, " ")
	arg = strings.TrimSpace(arg)
	switch cmd {
	case "/exit", "/quit", "/q":
		return true
	case "/help", "/?":
		for _, h := range slashHelp {
			c.Printf("  %-40s %s\n", c.Bold(h[0]), c.Dim(h[1]))
		}
		c.Printf("\n  %s\n", c.Dim(`multi-line input: end a line with \ or wrap text in """ lines`))
	case "/model":
		if arg == "" {
			c.Printf("model: %s\n", c.Cyan(orNone(a.Model())))
			if t := a.Primary(); t.Provider != nil {
				c.Printf("%s\n", c.Dim(fmt.Sprintf("context %s tokens · $%.2f/$%.2f per 1M in/out · tools %s", kTok(t.Model.ContextWindow), t.Model.InputPrice, t.Model.OutputPrice, t.Model.Tools)))
			}
			return
		}
		if err := a.SetModel(arg); err != nil {
			c.Error(err)
			return
		}
		r.save()
		c.Info("model → " + arg)
	case "/models":
		listModels(ctx, c, a.Cfg, false)
	case "/mode":
		if arg == "" {
			c.Printf("mode: %s\n", c.Yellow(a.Perms.Mode()))
			c.Printf("%s\n", c.Dim("plan: read-only · default: ask before edits/commands · accept-edits: edits auto, commands ask · auto: edits+commands auto (destructive always asks)"))
			return
		}
		if err := a.SetMode(arg); err != nil {
			c.Error(err)
			return
		}
		r.save()
		c.Info("mode → " + arg)
	case "/plan":
		if arg == "" {
			c.Error(errors.New("usage: /plan <task>"))
			return
		}
		prev := a.Perms.Mode()
		_ = a.SetMode("plan")
		r.turn(ctx, arg)
		_ = a.SetMode(prev)
		r.save()
		c.Info(fmt.Sprintf("back in %q mode — say \"go\" to implement the plan", prev))
	case "/review":
		diff, scope, err := reviewDiff(a.WS, arg, false)
		if err != nil {
			c.Error(err)
			return
		}
		if strings.TrimSpace(diff) == "" {
			c.Info("nothing to review")
			return
		}
		prev := a.Perms.Mode()
		_ = a.SetMode("plan")
		r.turn(ctx, agent.ReviewTask(diff, scope))
		_ = a.SetMode(prev)
		r.save()
	case "/diff":
		d := a.Env.Tracker.Diff(nil)
		if d == "" {
			c.Info("no changes in this session")
			return
		}
		c.Printf("%s", c.Diff(d, 0))
	case "/undo":
		files, err := a.Env.Tracker.Undo()
		if err != nil {
			c.Error(err)
			return
		}
		a.AddNote(fmt.Sprintf("<system-reminder>The user reverted your previous changes to: %s. Re-read these files before editing them again.</system-reminder>", strings.Join(files, ", ")))
		r.save()
		c.Info("reverted: " + strings.Join(files, ", "))
	case "/compact":
		tctx, done := r.sig.turn(ctx)
		err := a.Compact(tctx)
		done()
		if err != nil {
			c.Error(err)
			return
		}
		c.Info("conversation compacted")
	case "/status", "/cost":
		s := a.Session
		c.Printf("session  %s %s\n", s.ID, c.Dim(fmt.Sprintf("(%s)", s.Title)))
		c.Printf("model    %s · mode %s\n", orNone(s.Model), a.Perms.Mode())
		c.Printf("usage    %s in · %s cached · %s out · ~$%.4f\n", kTok(s.Usage.InputTokens+s.Usage.CacheWriteTokens), kTok(s.Usage.CacheReadTokens), kTok(s.Usage.OutputTokens), s.Cost)
		if t := a.Primary(); t.Model.ContextWindow > 0 && s.LastContextTokens > 0 {
			c.Printf("context  %s / %s tokens (%.0f%%)\n", kTok(s.LastContextTokens), kTok(t.Model.ContextWindow), 100*float64(s.LastContextTokens)/float64(t.Model.ContextWindow))
		}
		if ch := a.Env.Tracker.Changed(); len(ch) > 0 {
			c.Printf("changed  %s\n", strings.Join(ch, ", "))
		}
		c.Printf("audit    %s\n", c.Dim(a.Store.AuditPath()))
	case "/remember":
		if arg == "" {
			c.Error(errors.New("usage: /remember <note>"))
			return
		}
		if err := a.Store.Remember(arg); err != nil {
			c.Error(err)
			return
		}
		a.AddNote("<system-reminder>The user saved this to project memory (applies from now on): " + arg + "</system-reminder>")
		c.Info("saved to project memory (" + a.Store.MemoryPath() + ")")
	case "/memory":
		m := strings.TrimSpace(a.Store.Memory())
		if m == "" {
			c.Info("project memory is empty — add with /remember <note>")
			return
		}
		c.Printf("%s\n", m)
	case "/sessions":
		printSessions(c, a.Store, a.Session.ID)
	case "/resume":
		if arg == "" {
			c.Error(errors.New("usage: /resume <id>"))
			return
		}
		r.switchSession(arg)
	case "/new":
		r.switchSession("")
	case "/tools":
		printTools(c)
	default:
		c.Error(fmt.Errorf("unknown command %s (/help)", cmd))
	}
	return false
}

func (r *repl) save() {
	if err := r.a.Store.Save(r.a.Session); err != nil {
		r.con.Error(err)
	}
}

func (r *repl) switchSession(id string) {
	model, mode := r.a.Model(), r.a.Perms.Mode()
	if id != "" {
		model, mode = "", ""
	}
	a, err := newAgent(r.con, model, mode, id)
	if err != nil {
		r.con.Error(err)
		if a == nil {
			return
		}
	}
	r.a = a
	r.banner()
}

func printTools(c *ui.Console) {
	for _, t := range tools.Default().Sorted() {
		c.Printf("  %-12s %s\n", c.Bold(t.Name()), c.Dim(clipLine(t.Description(), 100)))
	}
}

func printSessions(c *ui.Console, st *session.Store, current string) {
	l, err := st.List()
	if err != nil {
		c.Error(err)
		return
	}
	if len(l) == 0 {
		c.Info("no sessions yet")
		return
	}
	for _, m := range l {
		mark := " "
		if m.ID == current {
			mark = "*"
		}
		status := m.Status
		if status == session.StatusInterrupted {
			status = c.Yellow(status)
		}
		c.Printf("%s %s  %-12s %-28s %3d turns  $%.4f  %s\n", mark, m.ID, status, clipLine(m.Model, 28), m.Turns, m.Cost, clipLine(m.Title, 50))
	}
}

func lastAssistantText(msgs []llm.Message) string {
	for i := len(msgs) - 1; i >= 0; i-- {
		if msgs[i].Role == llm.RoleAssistant {
			if t := strings.TrimSpace(msgs[i].Text()); t != "" {
				return t
			}
		}
	}
	return ""
}

func clipLine(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > n {
		return s[:n-1] + "…"
	}
	return s
}

func orNone(s string) string {
	if s == "" {
		return "(no model)"
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
