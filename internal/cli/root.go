// Package cli wires the cobra command tree.
package cli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"runtime"
	"strings"
	"sync"

	"github.com/spf13/cobra"

	"github.com/Abhinivesh2729/theepori/internal/agent"
	"github.com/Abhinivesh2729/theepori/internal/ui"
)

// Build info, set via -ldflags.
var (
	Version = "0.1.0-dev"
	Commit  = "none"
	Date    = "unknown"
)

type rootFlags struct {
	model  string
	mode   string
	yes    bool
	resume string
	cont   bool
}

func NewRoot() *cobra.Command {
	f := &rootFlags{}
	root := &cobra.Command{
		Use:   "tp [task]",
		Short: "Thee Pori (தீப்பொறி) — a local-first AI coding agent",
		Long: `Thee Pori (தீப்பொறி) is a coding agent for your terminal.

  tp                     start an interactive session in this repository
  tp "add input validation to the signup handler"
                         run one task and exit
  echo "fix the lint errors" | tp --mode accept-edits

Workflow: Understand → Plan → Modify → Test → Review. File edits and
commands follow your permission mode; destructive actions always ask.`,
		Args:          cobra.ArbitraryArgs,
		SilenceUsage:  true,
		SilenceErrors: true,
		Version:       Version,
		RunE: func(cmd *cobra.Command, args []string) error {
			mode := f.mode
			if f.yes && mode == "" {
				mode = "auto"
			}
			resume := f.resume
			if f.cont {
				resume = "latest"
			}
			task := strings.TrimSpace(strings.Join(args, " "))
			stdinTTY := ui.IsTerminal(os.Stdin)
			if task == "" && !stdinTTY {
				b, err := io.ReadAll(os.Stdin)
				if err != nil {
					return err
				}
				task = strings.TrimSpace(string(b))
				if task == "" {
					return errors.New("no task given on stdin")
				}
			}
			if task != "" {
				return runTask(cmd.Context(), task, f.model, mode, resume, stdinTTY)
			}
			return runInteractive(cmd.Context(), f.model, mode, resume)
		},
	}
	root.SetVersionTemplate(fmt.Sprintf("tp %s (commit %s, built %s, %s %s/%s)\n", Version, Commit, Date, runtime.Version(), runtime.GOOS, runtime.GOARCH))
	pf := root.Flags()
	pf.StringVarP(&f.model, "model", "m", "", "model as provider/model (e.g. anthropic/claude-opus-5-5)")
	pf.StringVar(&f.mode, "mode", "", "permission mode: plan | default | accept-edits | auto")
	pf.BoolVarP(&f.yes, "yes", "y", false, "shorthand for --mode auto (destructive actions still need approval)")
	pf.StringVarP(&f.resume, "resume", "r", "", "resume a session by id")
	pf.BoolVarP(&f.cont, "continue", "c", false, "resume the most recent session")

	root.AddCommand(
		initCmd(), configCmd(), providersCmd(), modelsCmd(), planCmd(), sessionCmd(),
		reviewCmd(), toolsCmd(), authCmd(), doctorCmd(), versionCmd(), memoryCmd(), dataCmd(), auditCmd(),
	)
	return root
}

// Execute runs the CLI and returns the process exit code.
func Execute() int {
	root := NewRoot()
	if err := root.ExecuteContext(context.Background()); err != nil {
		con := ui.NewConsole(false)
		con.Error(err)
		return 1
	}
	return 0
}

// signals routes Ctrl-C: cancel the running turn, else interrupt the prompt.
type signals struct {
	mu     sync.Mutex
	cancel context.CancelFunc
	con    *ui.Console
	stop   func()
}

func newSignals(con *ui.Console) *signals {
	s := &signals{con: con}
	ch := make(chan os.Signal, 1)
	signal.Notify(ch, os.Interrupt)
	done := make(chan struct{})
	go func() {
		for {
			select {
			case <-ch:
				s.mu.Lock()
				c := s.cancel
				s.mu.Unlock()
				if c != nil {
					c()
				} else {
					con.Interrupt()
				}
			case <-done:
				return
			}
		}
	}()
	s.stop = func() { signal.Stop(ch); close(done) }
	return s
}

// turn returns a cancellable context for one agent turn.
func (s *signals) turn(parent context.Context) (context.Context, func()) {
	ctx, cancel := context.WithCancel(parent)
	s.mu.Lock()
	s.cancel = cancel
	s.mu.Unlock()
	return ctx, func() {
		s.mu.Lock()
		s.cancel = nil
		s.mu.Unlock()
		cancel()
	}
}

func cwd() string {
	d, err := os.Getwd()
	if err != nil {
		return "."
	}
	return d
}

// newAgent builds an agent wired to the console.
func newAgent(con *ui.Console, model, mode, resume string) (*agent.Agent, error) {
	opts := agent.Options{Cwd: cwd(), Model: model, Mode: mode, Resume: resume, UI: con}
	if con.Interactive {
		opts.Approver = con
		opts.Ask = con.Ask
	}
	return agent.New(opts)
}

func runTask(ctx context.Context, task, model, mode, resume string, interactive bool) error {
	con := ui.NewConsole(interactive)
	if interactive {
		con.StartInput(os.Stdin)
	}
	a, err := newAgent(con, model, mode, resume)
	if err != nil {
		return err
	}
	if !interactive {
		switch a.Perms.Mode() {
		case "default":
			con.Info("non-interactive run: file edits and commands need approval and will be denied (use --mode accept-edits or --yes)")
		case "accept-edits":
			con.Info("non-interactive run: edits are allowed; commands need approval and will be denied (use --yes to allow them)")
		case "auto":
			con.Info("non-interactive run: destructive commands need approval and will be denied")
		}
	}
	sig := newSignals(con)
	defer sig.stop()
	tctx, done := sig.turn(ctx)
	defer done()
	err = a.Run(tctx, task)
	con.Info(fmt.Sprintf("session %s · resume with: tp -r %s", a.Session.ID, a.Session.ID))
	return err
}
