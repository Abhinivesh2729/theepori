package agent

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/providers"
	"github.com/Abhinivesh2729/theepori/internal/secrets"
	"github.com/Abhinivesh2729/theepori/internal/session"
	"github.com/Abhinivesh2729/theepori/internal/tools"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

type Options struct {
	Cwd      string
	Model    string // overrides config/session
	Mode     string // overrides config/session
	Resume   string // session id, "latest", or "" for a new session
	UI       UI
	Approver permission.Approver
	Ask      func(ctx context.Context, q string) (string, error)
}

// Project is the loaded per-repository context shared by commands.
type Project struct {
	Cfg   *config.Loaded
	WS    *workspace.Workspace
	Store *session.Store
}

// LoadProject detects the repo and loads config. A config error is returned
// together with the partially loaded project so `tp doctor` can report it.
func LoadProject(cwd string) (*Project, error) {
	ws := workspace.Detect(cwd, nil)
	cfg, err := config.Load(ws.Root)
	if cfg == nil {
		return nil, err
	}
	ws.Ignore = cfg.Context.Ignore
	return &Project{Cfg: cfg, WS: ws, Store: session.NewStore(ws.ProjectID())}, err
}

// New builds a ready-to-run agent with a new or resumed session.
func New(opts Options) (*Agent, error) {
	p, err := LoadProject(opts.Cwd)
	if err != nil {
		return nil, err
	}
	cfg := p.Cfg
	var sess *session.Session
	switch opts.Resume {
	case "":
	case "latest":
		if sess, err = p.Store.Latest(); err != nil {
			return nil, err
		}
	default:
		if sess, err = p.Store.Load(opts.Resume); err != nil {
			return nil, err
		}
	}
	if sess == nil {
		files := p.WS.Files()
		testCmd := cfg.TestCommand
		if testCmd == "" {
			testCmd = workspace.DetectTestCommand(p.WS.Root)
		}
		sess = &session.Session{
			ID:          session.NewID(),
			ProjectRoot: p.WS.Root,
			Created:     time.Now(),
			Status:      session.StatusIdle,
			Model:       cfg.ResolveDefaultModel(),
			Mode:        cfg.Permissions.Mode,
			System:      SystemPrompt(p.WS, files, p.WS.Instructions(cfg.Context.InstructionFiles), p.Store.Memory(), testCmd),
		}
	}
	if opts.Model != "" {
		sess.Model = opts.Model
	}
	if opts.Mode != "" {
		if !slices.Contains(config.Modes, opts.Mode) {
			return nil, fmt.Errorf("invalid mode %q (one of: %s)", opts.Mode, strings.Join(config.Modes, ", "))
		}
		sess.Mode = opts.Mode
	}
	if sess.Mode == "" {
		sess.Mode = cfg.Permissions.Mode
	}

	redact := secrets.New(providers.KeyEnvVars(cfg)...)
	redact.Add(providers.StoredKeys(cfg)...)
	testCmd := cfg.TestCommand
	if testCmd == "" {
		testCmd = workspace.DetectTestCommand(p.WS.Root)
	}
	a := &Agent{
		Cfg:     cfg,
		WS:      p.WS,
		Tools:   tools.Default(),
		Store:   p.Store,
		Session: sess,
		UI:      opts.UI,
		Audit:   permission.NewAuditLog(p.Store.AuditPath(), redact),
		Perms: permission.NewEngine(permission.Policy{
			Mode:          sess.Mode,
			AllowCommands: cfg.Permissions.AllowCommands,
			DenyCommands:  cfg.Permissions.DenyCommands,
			DenyPaths:     cfg.Permissions.DenyPaths,
		}, opts.Approver),
	}
	a.Env = &tools.Env{
		WS:           p.WS,
		Redact:       redact,
		Tracker:      tools.NewTracker(p.WS.Root, &sess.Tracker),
		MaxFileBytes: cfg.Context.MaxFileBytes,
		TestCommand:  testCmd,
		StripEnv:     providers.KeyEnvVars(cfg),
		Ask:          opts.Ask,
		OnPlan: func(steps []tools.PlanStep, note string) {
			a.Session.Plan = steps
			a.UI.Plan(steps, note)
		},
	}
	if sess.Model != "" {
		if err := a.SetModel(sess.Model); err != nil {
			return a, err
		}
	} else {
		return a, (&noModelError{})
	}
	return a, nil
}

type noModelError struct{}

func (noModelError) Error() string {
	return "no model available: set ANTHROPIC_API_KEY or OPENAI_API_KEY, configure Thinai (`tp providers discover`), or run `tp models use <provider/model>`"
}

// SetMode changes the permission mode for the session.
func (a *Agent) SetMode(mode string) error {
	if !slices.Contains(config.Modes, mode) {
		return fmt.Errorf("invalid mode %q (one of: %s)", mode, strings.Join(config.Modes, ", "))
	}
	a.Perms.SetMode(mode)
	a.Session.Mode = mode
	return nil
}
