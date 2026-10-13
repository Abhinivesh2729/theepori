package cli

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/spf13/cobra"
	"gopkg.in/yaml.v3"

	"github.com/Abhinivesh2729/theepori/internal/agent"
	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/llm/ollama"
	"github.com/Abhinivesh2729/theepori/internal/permission"
	"github.com/Abhinivesh2729/theepori/internal/providers"
	"github.com/Abhinivesh2729/theepori/internal/secrets"
	"github.com/Abhinivesh2729/theepori/internal/session"
	"github.com/Abhinivesh2729/theepori/internal/tools"
	"github.com/Abhinivesh2729/theepori/internal/ui"
	"github.com/Abhinivesh2729/theepori/internal/workspace"
)

func console() *ui.Console { return ui.NewConsole(false) }

// --------------------------------------------------------------------- init

const projectTemplate = `# Thee Pori project config — commit this file.
# Project config can only TIGHTEN permissions and cannot define providers.
# Global settings: %s

# default_model: anthropic/claude-opus-5-5
test_command: %q

permissions:
  # plan | default | accept-edits | auto  (stricter than your global mode only)
  mode: default
  deny_commands: []     # e.g. ["npm publish*", "make deploy*"]
  deny_paths: []        # e.g. ["migrations/**"]

context:
  ignore: []            # extra globs hidden from the agent, e.g. ["**/*.min.js"]
`

const globalTemplate = `# Thee Pori global config (credentials come from environment variables only).
# Docs: tp config --help

# default_model: anthropic/claude-opus-5-5
# fallback_models: [openai/gpt-5]

providers:
  anthropic:
    type: anthropic
    api_key_env: ANTHROPIC_API_KEY
  openai:
    type: openai
    base_url: https://api.openai.com/v1
    api_key_env: OPENAI_API_KEY
  thinai:
    type: thinai          # Thinai Android app (Ollama-compatible) on your Wi-Fi
    # base_url: http://192.168.1.36:11434   # or run: tp providers discover --save
  # groq:
  #   type: openai-compatible
  #   base_url: https://api.groq.com/openai/v1
  #   api_key_env: GROQ_API_KEY
  #   models:
  #     llama-3.3-70b-versatile: {context_window: 128000, input_price: 0.59, output_price: 0.79}

permissions:
  mode: default           # plan | default | accept-edits | auto
  allow_commands: []      # run without asking, e.g. ["go test *", "npm run lint"]
  deny_commands: []

agent:
  effort: medium          # Claude effort: low | medium | high | xhigh | max
  max_steps: 60
`

func initCmd() *cobra.Command {
	var force bool
	cmd := &cobra.Command{
		Use:   "init",
		Short: "Initialize project configuration (.theepori/config.yaml)",
		RunE: func(cmd *cobra.Command, args []string) error {
			c := console()
			ws := workspace.Detect(cwd(), nil)
			path := config.ProjectPath(ws.Root)
			if _, err := os.Stat(path); err == nil && !force {
				return fmt.Errorf("%s already exists (use --force to overwrite)", path)
			}
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				return err
			}
			test := workspace.DetectTestCommand(ws.Root)
			if err := os.WriteFile(path, []byte(fmt.Sprintf(projectTemplate, config.GlobalPath(), test)), 0o644); err != nil {
				return err
			}
			c.Printf("%s %s\n", c.Green("✔"), path)
			if test != "" {
				c.Printf("  detected test command: %s\n", test)
			}
			if _, err := os.Stat(config.GlobalPath()); errors.Is(err, os.ErrNotExist) {
				if err := os.MkdirAll(config.AppDir(), 0o700); err == nil {
					if err := os.WriteFile(config.GlobalPath(), []byte(globalTemplate), 0o600); err == nil {
						c.Printf("%s %s %s\n", c.Green("✔"), config.GlobalPath(), c.Dim("(global config)"))
					}
				}
			}
			if len(ws.Instructions([]string{"AGENTS.md", "CLAUDE.md"})) == 0 {
				c.Printf("%s\n", c.Dim("tip: add an AGENTS.md with build/test commands and conventions — tp reads it every session"))
			}
			c.Printf("%s\n", c.Dim("next: tp auth   (check credentials)   ·   tp doctor   ·   tp"))
			return nil
		},
	}
	cmd.Flags().BoolVar(&force, "force", false, "overwrite an existing project config")
	return cmd
}

// ------------------------------------------------------------------- config

func configCmd() *cobra.Command {
	var project bool
	target := func() (string, error) {
		if !project {
			return config.GlobalPath(), nil
		}
		ws := workspace.Detect(cwd(), nil)
		return config.ProjectPath(ws.Root), nil
	}
	cmd := &cobra.Command{
		Use:   "config",
		Short: "Show and edit settings (global or --project)",
		Long: `Settings merge in order: built-in defaults < global < project < flags/env.

Global:  ` + config.GlobalPath() + `
Project: <repo>/.theepori/config.yaml (may only tighten permissions; cannot define providers)

Keys: default_model, fallback_models, providers.<name>.{type,base_url,api_key_env,timeout,headers,num_ctx,models},
      permissions.{mode,allow_commands,deny_commands,deny_paths}, agent.{effort,max_steps,max_tokens,max_retries,compact_at},
      context.{ignore,instruction_files,max_file_bytes}, test_command`,
		RunE: func(cmd *cobra.Command, args []string) error { return showConfig() },
	}
	cmd.PersistentFlags().BoolVar(&project, "project", false, "operate on the project config instead of the global one")
	cmd.AddCommand(
		&cobra.Command{Use: "show", Short: "Print the effective configuration", RunE: func(*cobra.Command, []string) error { return showConfig() }},
		&cobra.Command{Use: "path", Short: "Print config file paths", RunE: func(*cobra.Command, []string) error {
			ws := workspace.Detect(cwd(), nil)
			fmt.Printf("global:  %s\nproject: %s\ndata:    %s\n", config.GlobalPath(), config.ProjectPath(ws.Root), config.AppDir())
			return nil
		}},
		&cobra.Command{Use: "get <key>", Short: "Print one effective value", Args: cobra.ExactArgs(1), RunE: func(_ *cobra.Command, args []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			v, ok := lookupKey(p.Cfg.Config, args[0])
			if !ok {
				return fmt.Errorf("%s is not set", args[0])
			}
			b, _ := yaml.Marshal(v)
			fmt.Print(string(b))
			return nil
		}},
		&cobra.Command{Use: "set <key> <value>", Short: "Set a value (YAML syntax: true, 5, [a, b])", Args: cobra.ExactArgs(2), RunE: func(_ *cobra.Command, args []string) error {
			path, err := target()
			if err != nil {
				return err
			}
			if err := config.Set(path, args[0], args[1]); err != nil {
				return err
			}
			if _, err := agent.LoadProject(cwd()); err != nil {
				fmt.Fprintf(os.Stderr, "warning: %v\n", err)
			}
			fmt.Printf("%s = %s  (%s)\n", args[0], args[1], path)
			return nil
		}},
		&cobra.Command{Use: "unset <key>", Short: "Remove a value", Args: cobra.ExactArgs(1), RunE: func(_ *cobra.Command, args []string) error {
			path, err := target()
			if err != nil {
				return err
			}
			return config.Unset(path, args[0])
		}},
		&cobra.Command{Use: "edit", Short: "Open the config in $EDITOR", RunE: func(*cobra.Command, []string) error {
			path, err := target()
			if err != nil {
				return err
			}
			if _, err := os.Stat(path); errors.Is(err, os.ErrNotExist) {
				_ = os.MkdirAll(filepath.Dir(path), 0o700)
				tmpl := globalTemplate
				if project {
					tmpl = fmt.Sprintf(projectTemplate, config.GlobalPath(), "")
				}
				_ = os.WriteFile(path, []byte(tmpl), 0o600)
			}
			editor := os.Getenv("VISUAL")
			if editor == "" {
				editor = os.Getenv("EDITOR")
			}
			if editor == "" {
				editor = "vi"
			}
			e := exec.Command("sh", "-c", editor+` "$0"`, path)
			e.Stdin, e.Stdout, e.Stderr = os.Stdin, os.Stdout, os.Stderr
			if err := e.Run(); err != nil {
				return err
			}
			if _, err := agent.LoadProject(cwd()); err != nil {
				return err
			}
			fmt.Println("config is valid")
			return nil
		}},
	)
	return cmd
}

func showConfig() error {
	p, err := agent.LoadProject(cwd())
	if p == nil {
		return err
	}
	c := console()
	c.Printf("%s\n", c.Dim("# global:  "+p.Cfg.GlobalPath+existsNote(p.Cfg.GlobalPath)))
	pp := config.ProjectPath(p.WS.Root)
	c.Printf("%s\n", c.Dim("# project: "+pp+existsNote(pp)))
	for _, w := range p.Cfg.Warnings {
		c.Printf("%s\n", c.Yellow("# ! "+w))
	}
	b, _ := yaml.Marshal(p.Cfg.Config)
	c.Printf("%s", b)
	c.Printf("%s\n", c.Dim("# effective default model: "+orNone(p.Cfg.ResolveDefaultModel())))
	return err
}

func existsNote(p string) string {
	if _, err := os.Stat(p); err != nil {
		return " (not present)"
	}
	return ""
}

func lookupKey(cfg config.Config, key string) (any, bool) {
	b, _ := yaml.Marshal(cfg)
	var m any
	_ = yaml.Unmarshal(b, &m)
	for _, k := range strings.Split(key, ".") {
		mm, ok := m.(map[string]any)
		if !ok {
			return nil, false
		}
		if m, ok = mm[k]; !ok {
			return nil, false
		}
	}
	return m, true
}

// ---------------------------------------------------------------- providers

func providersCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "providers",
		Short: "List and manage LLM providers",
		RunE:  func(*cobra.Command, []string) error { return listProviders() },
	}
	var typ, base, keyEnv, preset string
	var noTest bool
	add := &cobra.Command{
		Use:   "add <name>",
		Short: "Add a provider from a preset (or a custom endpoint) to the global config",
		Long: `Presets need no flags: tp providers add groq
Presets also work without adding them — just export the key and use
tp -m groq/<model>. Adding saves the provider so it shows up in listings and
lets you override its base URL or key variable.

See all presets: tp providers presets`,
		Example: `  tp providers add groq                                  # preset: base URL + GROQ_API_KEY
  tp providers add work-ai --preset openai --api-key-env WORK_OPENAI_KEY
  tp providers add local --type openai-compatible --base-url http://localhost:8000/v1
  tp providers add phone --type thinai --base-url 192.168.1.36`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			name := args[0]
			if preset == "" && typ == "" {
				if _, ok := config.LookupPreset(name); ok {
					preset = name
				} else {
					return fmt.Errorf("%q is not a preset — use --preset <name> or --type (see `tp providers presets`)", name)
				}
			}
			if preset != "" {
				if _, ok := config.LookupPreset(preset); !ok {
					return fmt.Errorf("unknown preset %q (see `tp providers presets`)", preset)
				}
			}
			path := config.GlobalPath()
			set := func(k, v string) error {
				if v == "" {
					return nil
				}
				return config.Set(path, "providers."+name+"."+k, v)
			}
			if typ == "thinai" || typ == "ollama" {
				base = ollama.NormalizeBaseURL(base)
			}
			presetField := preset
			if preset == name {
				presetField = "" // a preset's own name resolves to it implicitly
			}
			fields := [][2]string{{"preset", presetField}, {"type", typ}, {"base_url", base}, {"api_key_env", keyEnv}}
			if presetField == "" && typ == "" && base == "" && keyEnv == "" {
				// Plain `add <preset>`: record it with its type so it appears in listings.
				fields = [][2]string{{"type", mustPreset(preset).Type}}
			}
			for _, kv := range fields {
				if err := set(kv[0], kv[1]); err != nil {
					return err
				}
			}
			p, err := agent.LoadProject(cwd())
			if p == nil || err != nil {
				return err
			}
			c := console()
			pr, _ := p.Cfg.Provider(name)
			c.Printf("%s provider %s saved %s\n", c.Green("✔"), c.Bold(name), c.Dim("("+path+")"))
			c.Printf("  type %s · %s\n", pr.Type, orDefault(pr.BaseURL))
			def := ""
			if preset != "" {
				def = mustPreset(preset).DefaultModel
			}
			switch {
			case pr.NeedsKey() && pr.APIKey() == "":
				c.Printf("  %s %s is not set\n", c.Yellow("!"), pr.APIKeyEnv)
				if u := mustPreset(preset).KeyURL; u != "" {
					c.Printf("    get a key: %s\n", u)
				}
				c.Printf("    then add to ~/.zshrc: export %s=...\n", pr.APIKeyEnv)
			case !noTest && !((pr.Type == "thinai" || pr.Type == "ollama") && pr.BaseURL == ""):
				testProvider(cmd.Context(), p.Cfg, name)
			}
			if def != "" {
				c.Printf("  use it: tp -m %s/%s   or make it the default: tp models use %s/%s\n", name, def, name, def)
			} else {
				c.Printf("  models: tp models --remote   then: tp models use %s/<model>\n", name)
			}
			return nil
		},
	}
	add.Flags().StringVar(&preset, "preset", "", "start from a preset (see `tp providers presets`)")
	add.Flags().StringVar(&typ, "type", "", "custom provider: anthropic | openai | openai-compatible | thinai | ollama")
	add.Flags().StringVar(&base, "base-url", "", "API base URL (overrides the preset)")
	add.Flags().StringVar(&keyEnv, "api-key-env", "", "environment variable holding the API key (overrides the preset)")
	add.Flags().BoolVar(&noTest, "no-test", false, "skip the live connectivity test")

	presetsList := &cobra.Command{
		Use:   "presets",
		Short: "List built-in provider presets",
		RunE: func(*cobra.Command, []string) error {
			c := console()
			for _, pr := range config.Presets() {
				status := ""
				switch {
				case pr.Local:
					status = c.Dim("local, no key")
				case pr.KeySet():
					status = c.Green("✔ " + pr.APIKeyEnv + " — ready")
				default:
					status = c.Dim(pr.APIKeyEnv + " not set")
				}
				base := pr.BaseURL
				if base == "" {
					base = map[bool]string{true: "(tp providers discover)", false: "(default)"}[pr.Local]
				}
				c.Printf("  %-11s %-18s %-56s %s\n", c.Bold(pr.Name), pr.Type, base, status)
			}
			c.Printf("\n%s\n", c.Dim("ready presets work without adding: tp -m groq/<model> · save one: tp providers add <preset>"))
			return nil
		},
	}
	remove := &cobra.Command{
		Use: "remove <name>", Short: "Remove a provider from the global config", Args: cobra.ExactArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			if err := config.Unset(config.GlobalPath(), "providers."+args[0]); err != nil {
				return err
			}
			fmt.Printf("removed %q (built-in providers fall back to defaults)\n", args[0])
			return nil
		},
	}
	test := &cobra.Command{
		Use: "test [name]", Short: "Check connectivity, credentials and a tiny completion",
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			names := args
			if len(names) == 0 {
				names = p.Cfg.UsableNames()
			}
			failed := false
			for _, n := range names {
				if !testProvider(cmd.Context(), p.Cfg, n) {
					failed = true
				}
			}
			if failed {
				return errors.New("some providers failed")
			}
			return nil
		},
	}
	var save bool
	discover := &cobra.Command{
		Use:   "discover",
		Short: "Find Thinai (or Ollama) servers on this machine and your Wi-Fi",
		Long:  "Scans $THINAI_HOST, localhost and the local /24 subnet on port 11434. In the Thinai app: Server tab → Start → Share on local network.",
		RunE: func(cmd *cobra.Command, _ []string) error {
			c := console()
			c.Printf("%s\n", c.Dim("scanning port 11434 on localhost and your local subnet…"))
			ctx, cancel := context.WithTimeout(cmd.Context(), 20*time.Second)
			defer cancel()
			servers := ollama.Discover(ctx, 0, 0)
			if len(servers) == 0 {
				return errors.New("no Thinai/Ollama server found — in Thinai: Server tab → Start → enable \"Share on local network\"; keep both devices on the same Wi-Fi, or set THINAI_HOST")
			}
			for _, s := range servers {
				kind := "ollama"
				if s.Thinai {
					kind = "thinai"
				}
				c.Printf("%s %s %s\n", c.Green("✔"), s.BaseURL, c.Dim("("+kind+")"))
			}
			if save {
				s := servers[0]
				name := "thinai"
				if !s.Thinai {
					name = "ollama"
					if err := config.Set(config.GlobalPath(), "providers.ollama.type", "ollama"); err != nil {
						return err
					}
				}
				if err := config.Set(config.GlobalPath(), "providers."+name+".base_url", s.BaseURL); err != nil {
					return err
				}
				c.Printf("saved providers.%s.base_url = %s — try: tp -m %s/loaded\n", name, s.BaseURL, name)
			}
			return nil
		},
	}
	discover.Flags().BoolVar(&save, "save", false, "save the first server found to the global config")
	cmd.AddCommand(&cobra.Command{Use: "list", Short: "List providers", RunE: func(*cobra.Command, []string) error { return listProviders() }}, add, presetsList, remove, test, discover)
	return cmd
}

func listProviders() error {
	p, err := agent.LoadProject(cwd())
	if p == nil {
		return err
	}
	c := console()
	def := p.Cfg.ResolveDefaultModel()
	for _, name := range p.Cfg.UsableNames() {
		pr, _ := p.Cfg.Provider(name)
		cred := c.Dim("no key needed")
		switch {
		case pr.APIKeyEnv != "" && pr.APIKey() != "":
			cred = c.Green("✔ " + pr.APIKeyEnv)
		case pr.NeedsKey():
			cred = c.Red("✗ " + pr.APIKeyEnv + " not set")
		}
		base := pr.BaseURL
		if base == "" {
			base = "(default)"
			if pr.Type == "thinai" || pr.Type == "ollama" {
				base = c.Yellow("(not set — tp providers discover)")
			}
		}
		marker := " "
		if strings.HasPrefix(def, name+"/") {
			marker = c.Cyan("*")
		}
		c.Printf("%s %-12s %-18s %-40s %s\n", marker, c.Bold(name), pr.Type, base, cred)
	}
	c.Printf("%s\n", c.Dim("* = provider of the default model ("+orNone(def)+") · more: tp providers presets"))
	return err
}

func testProvider(ctx context.Context, cfg *config.Loaded, name string) bool {
	c := console()
	pr, _ := cfg.Provider(name)
	if pr.NeedsKey() && pr.APIKey() == "" {
		c.Printf("%s %-12s %s\n", c.Dim("-"), name, c.Dim("skipped: "+pr.APIKeyEnv+" not set"))
		return true
	}
	if (pr.Type == "thinai" || pr.Type == "ollama") && pr.BaseURL == "" {
		c.Printf("%s %-12s %s\n", c.Dim("-"), name, c.Dim("skipped: no base_url (tp providers discover --save)"))
		return true
	}
	prov, err := providers.Build(cfg, name)
	if err != nil {
		c.Printf("%s %-12s %v\n", c.Red("✗"), name, err)
		return false
	}
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	start := time.Now()
	models, err := prov.ListModels(ctx)
	if err != nil {
		c.Printf("%s %-12s list models: %v\n", c.Red("✗"), name, err)
		return false
	}
	model := ""
	if strings.HasPrefix(cfg.ResolveDefaultModel(), name+"/") {
		_, model = llm.ParseRef(cfg.ResolveDefaultModel())
	}
	if model == "" || model == "loaded" {
		// Preset suggestion, else the provider's own default, else whatever it lists.
		presetName := cfg.Providers[name].Preset
		if presetName == "" {
			presetName = name
		}
		model = mustPreset(presetName).DefaultModel
		if model == "" || model == "loaded" {
			model = llm.DefaultModelFor(pr.Type) // "" for compatible/local types
		}
		if model == "" && len(models) > 0 {
			model = models[0].ID
		}
	}
	_, info, _ := cfg.Model(name + "/" + model)
	resp, err := prov.Chat(ctx, llm.Request{Model: model, Messages: []llm.Message{llm.UserText("Reply with exactly: OK")}, MaxTokens: 2048, Effort: "low"}, nil)
	if err != nil {
		c.Printf("%s %-12s %d models, chat with %s failed: %v\n", c.Red("✗"), name, len(models), model, err)
		return false
	}
	c.Printf("%s %-12s %d models · %s replied %q in %s · %d tokens (~$%.5f)\n", c.Green("✔"), name, len(models), model,
		clipLine(resp.Message.Text(), 20), time.Since(start).Round(10*time.Millisecond), resp.Usage.InputTokens+resp.Usage.OutputTokens, llm.Cost(info, resp.Usage))
	return true
}

// ------------------------------------------------------------------- models

func modelsCmd() *cobra.Command {
	var remote bool
	cmd := &cobra.Command{
		Use:   "models",
		Short: "List and select models",
		RunE: func(cmd *cobra.Command, _ []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			listModels(cmd.Context(), console(), p.Cfg, remote)
			return err
		},
	}
	cmd.Flags().BoolVar(&remote, "remote", false, "query each provider's live model list")
	var project bool
	use := &cobra.Command{
		Use:   "use <provider/model>",
		Short: "Set the default model (global, or --project)",
		Args:  cobra.ExactArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			if _, _, err := p.Cfg.Model(args[0]); err != nil {
				return err
			}
			path := config.GlobalPath()
			if project {
				path = config.ProjectPath(p.WS.Root)
			}
			if err := config.Set(path, "default_model", args[0]); err != nil {
				return err
			}
			fmt.Printf("default model → %s (%s)\n", args[0], path)
			if _, err := providers.Target(p.Cfg, args[0]); err != nil {
				fmt.Printf("warning: %v\n", err)
			}
			return nil
		},
	}
	use.Flags().BoolVar(&project, "project", false, "set for this project only")
	cmd.AddCommand(use)
	return cmd
}

func listModels(ctx context.Context, c *ui.Console, cfg *config.Loaded, remote bool) {
	def := cfg.ResolveDefaultModel()
	for _, name := range cfg.UsableNames() {
		pr, _ := cfg.Provider(name)
		var models []llm.ModelInfo
		ctype := pr.Type
		if ctype == "openai-compatible" {
			ctype = "openai"
		}
		live := remote || pr.Type == "thinai" || pr.Type == "ollama" || pr.Type == "openai-compatible"
		if live {
			if prov, err := providers.Build(cfg, name); err == nil {
				lctx, cancel := context.WithTimeout(ctx, 15*time.Second)
				models, err = prov.ListModels(lctx)
				cancel()
				if err != nil {
					c.Printf("%s %s\n", c.Bold(name), c.Red(clipLine(err.Error(), 120)))
					continue
				}
			} else {
				c.Printf("%s %s\n", c.Bold(name), c.Dim(clipLine(err.Error(), 120)))
				continue
			}
		} else {
			models = llm.CatalogModels(ctype)
		}
		c.Printf("%s\n", c.Bold(name))
		for id := range pr.Models {
			found := false
			for _, m := range models {
				found = found || m.ID == id
			}
			if !found {
				models = append(models, llm.ModelInfo{ID: id})
			}
		}
		for _, m := range models {
			ref := name + "/" + m.ID
			_, info, _ := cfg.Model(ref)
			mark := " "
			if ref == def {
				mark = c.Cyan("*")
			}
			price := c.Dim("price n/a")
			if info.InputPrice > 0 || info.OutputPrice > 0 {
				price = fmt.Sprintf("$%.2f/$%.2f", info.InputPrice, info.OutputPrice)
			}
			c.Printf(" %s %-44s ctx %-6s %-16s tools:%s\n", mark, ref, kTok(info.ContextWindow), price, info.Tools)
		}
	}
	c.Printf("%s\n", c.Dim("* default · prices are USD per 1M input/output tokens (estimates) · switch: tp models use <ref> or /model"))
}

// --------------------------------------------------------------------- plan

func planCmd() *cobra.Command {
	var model string
	return &cobra.Command{
		Use:   "plan <task>",
		Short: "Investigate read-only and produce an implementation plan",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			interactive := ui.IsTerminal(os.Stdin)
			con := ui.NewConsole(interactive)
			if interactive {
				con.StartInput(os.Stdin)
			}
			a, err := newAgent(con, model, "plan", "")
			if err != nil {
				return err
			}
			sig := newSignals(con)
			defer sig.stop()
			ctx, done := sig.turn(cmd.Context())
			defer done()
			if err := a.Run(ctx, strings.Join(args, " ")); err != nil {
				return err
			}
			con.Info(fmt.Sprintf("plan saved in session %s — implement with: tp -r %s --mode default", a.Session.ID, a.Session.ID))
			return nil
		},
	}
}

// ------------------------------------------------------------------ session

func sessionCmd() *cobra.Command {
	cmd := &cobra.Command{Use: "session", Short: "List, resume, inspect and delete saved sessions"}
	store := func() (*session.Store, error) {
		p, err := agent.LoadProject(cwd())
		if p == nil {
			return nil, err
		}
		return p.Store, nil
	}
	var all bool
	del := &cobra.Command{
		Use: "delete [id]", Short: "Delete a session (or --all for this project)",
		RunE: func(_ *cobra.Command, args []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			if all {
				n, err := st.DeleteAll()
				if err != nil {
					return err
				}
				fmt.Printf("deleted %d sessions\n", n)
				return nil
			}
			if len(args) != 1 {
				return errors.New("usage: tp session delete <id> | --all")
			}
			id, err := st.Delete(args[0])
			if err != nil {
				return err
			}
			fmt.Printf("deleted %s\n", id)
			return nil
		},
	}
	del.Flags().BoolVar(&all, "all", false, "delete every session of this project")
	cmd.AddCommand(
		&cobra.Command{Use: "list", Short: "List sessions in this project", RunE: func(*cobra.Command, []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			printSessions(console(), st, "")
			return nil
		}},
		&cobra.Command{Use: "resume [id]", Short: "Resume a session interactively (latest if no id)", Args: cobra.MaximumNArgs(1), RunE: func(cmd *cobra.Command, args []string) error {
			id := "latest"
			if len(args) == 1 {
				id = args[0]
			}
			return runInteractive(cmd.Context(), "", "", id)
		}},
		&cobra.Command{Use: "show <id>", Short: "Print a session transcript", Args: cobra.ExactArgs(1), RunE: func(_ *cobra.Command, args []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			s, err := st.Load(args[0])
			if err != nil {
				return err
			}
			printTranscript(console(), s)
			return nil
		}},
		del,
	)
	return cmd
}

func printTranscript(c *ui.Console, s *session.Session) {
	c.Printf("%s %s\n%s\n\n", c.Bold(s.ID), s.Title, c.Dim(fmt.Sprintf("%s · %s · %d turns · ~$%.4f · %s", s.Model, s.Status, s.Turns, s.Cost, s.ProjectRoot)))
	for _, m := range s.Messages {
		for _, p := range m.Parts {
			switch p.Type {
			case llm.PartText:
				if m.Role == llm.RoleUser {
					c.Printf("%s %s\n\n", c.Bold("›"), p.Text)
				} else {
					c.Printf("%s\n\n", p.Text)
				}
			case llm.PartToolCall:
				c.Printf("%s %s %s\n", c.Cyan("●"), p.ToolCall.Name, c.Dim(clipLine(string(p.ToolCall.Input), 120)))
			case llm.PartToolResult:
				line := clipLine(p.ToolResult.Content, 120)
				if p.ToolResult.IsError {
					line = c.Red(line)
				} else {
					line = c.Dim(line)
				}
				c.Printf("  ⎿ %s\n", line)
			}
		}
	}
}

// ------------------------------------------------------------------- review

func reviewCmd() *cobra.Command {
	var base, model string
	var staged bool
	cmd := &cobra.Command{
		Use:   "review",
		Short: "Review code changes (working tree, --staged, or --base <ref>)",
		RunE: func(cmd *cobra.Command, _ []string) error {
			interactive := ui.IsTerminal(os.Stdin)
			con := ui.NewConsole(interactive)
			a, err := newAgent(con, model, "plan", "")
			if err != nil {
				return err
			}
			diff, scope, err := reviewDiff(a.WS, base, staged)
			if err != nil {
				return err
			}
			if strings.TrimSpace(diff) == "" {
				con.Info("nothing to review: " + scope + " has no changes")
				return nil
			}
			sig := newSignals(con)
			defer sig.stop()
			ctx, done := sig.turn(cmd.Context())
			defer done()
			return a.Run(ctx, agent.ReviewTask(diff, scope))
		},
	}
	cmd.Flags().StringVar(&base, "base", "", "review commits since this ref (e.g. main)")
	cmd.Flags().BoolVar(&staged, "staged", false, "review only staged changes")
	cmd.Flags().StringVarP(&model, "model", "m", "", "model to use")
	return cmd
}

// reviewDiff collects the diff to review, including untracked files.
func reviewDiff(ws *workspace.Workspace, base string, staged bool) (string, string, error) {
	if !ws.IsGit {
		return "", "", errors.New("review needs a git repository")
	}
	var args []string
	scope := "uncommitted changes (working tree + untracked files)"
	switch {
	case staged:
		args, scope = []string{"diff", "--cached"}, "staged changes"
	case base != "":
		args, scope = []string{"diff", base + "...HEAD"}, "commits on HEAD since "+base
	default:
		args = []string{"diff", "HEAD"}
		if _, err := workspace.Git(ws.Root, "rev-parse", "--verify", "HEAD"); err != nil {
			args = []string{"diff", "--cached"} // no commits yet
		}
	}
	out, err := workspace.Git(ws.Root, args...)
	if err != nil {
		return "", "", err
	}
	if base == "" && !staged {
		untracked, _ := workspace.Git(ws.Root, "ls-files", "--others", "--exclude-standard")
		for _, f := range strings.Split(strings.TrimSpace(untracked), "\n") {
			if f == "" || workspace.Sensitive(f, nil) || ws.Ignored(f) {
				continue
			}
			b, err := os.ReadFile(filepath.Join(ws.Root, f))
			if err != nil || len(b) > 200_000 || strings.ContainsRune(string(b[:min(len(b), 8000)]), 0) {
				continue
			}
			out += tools.UnifiedDiff(f, "", string(b), false, true)
		}
	}
	return secrets.New().Redact(out), scope, nil
}

// -------------------------------------------------------------------- tools

func toolsCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "tools",
		Short: "List available tools and their risk levels",
		RunE: func(*cobra.Command, []string) error {
			c := console()
			risk := map[string]string{
				"read_file": "read", "list_dir": "read", "glob": "read", "grep": "read", "git_status": "read", "git_diff": "read",
				"write_file": "write", "edit_file": "write", "run_command": "exec*", "run_tests": "exec", "update_plan": "none", "ask_user": "none",
			}
			for _, t := range tools.Default().Sorted() {
				c.Printf("  %-12s %-6s %s\n", c.Bold(t.Name()), risk[t.Name()], c.Dim(clipLine(t.Description(), 90)))
			}
			c.Printf("\n%s\n", c.Dim("risk: read runs freely · write/exec follow your mode · *commands are classified: read-only commands run freely, destructive ones (rm -rf, git push, deploys…) always ask"))
			return nil
		},
	}
}

// --------------------------------------------------------------------- auth

func authCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "auth",
		Short: "Show credential status (keys are read from environment variables only)",
		Long: `tp never stores API keys. Each provider reads its key from the environment
variable named by providers.<name>.api_key_env. Keys are masked in output,
redacted from logs/sessions, and removed from the environment of commands the
agent runs.`,
		RunE: func(*cobra.Command, []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			c := console()
			missing := false
			for _, name := range p.Cfg.UsableNames() {
				pr, _ := p.Cfg.Provider(name)
				switch {
				case pr.APIKeyEnv == "":
					c.Printf("  %-12s %s\n", name, c.Dim("no credential configured"))
				case pr.APIKey() != "":
					c.Printf("  %-12s %s %s\n", name, c.Green("✔ "+pr.APIKeyEnv), c.Dim(secrets.Mask(pr.APIKey())))
				case pr.NeedsKey():
					missing = true
					c.Printf("  %-12s %s\n", name, c.Red("✗ "+pr.APIKeyEnv+" not set"))
				default:
					c.Printf("  %-12s %s\n", name, c.Dim(pr.APIKeyEnv+" not set (optional)"))
				}
			}
			if missing {
				c.Printf("\n%s\n", c.Dim("set keys in your shell profile (~/.zshrc), e.g.:\n  export ANTHROPIC_API_KEY=sk-ant-...\nthen verify with: tp auth check"))
			}
			return err
		},
	}
	cmd.AddCommand(&cobra.Command{
		Use:   "check",
		Short: "Verify each configured key against its provider",
		RunE: func(cmd *cobra.Command, _ []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			c := console()
			bad := false
			for _, name := range p.Cfg.UsableNames() {
				pr, _ := p.Cfg.Provider(name)
				if pr.APIKey() == "" {
					continue
				}
				prov, err := providers.Build(p.Cfg, name)
				if err == nil {
					ctx, cancel := context.WithTimeout(cmd.Context(), 20*time.Second)
					_, err = prov.ListModels(ctx)
					cancel()
				}
				if err != nil {
					bad = true
					c.Printf("  %s %-12s %v\n", c.Red("✗"), name, err)
					continue
				}
				c.Printf("  %s %-12s key accepted\n", c.Green("✔"), name)
			}
			if bad {
				return errors.New("some credentials failed")
			}
			return nil
		},
	})
	return cmd
}

// ------------------------------------------------------------------- doctor

func doctorCmd() *cobra.Command {
	var offline bool
	cmd := &cobra.Command{
		Use:   "doctor",
		Short: "Diagnose configuration, credentials and connectivity",
		RunE: func(cmd *cobra.Command, _ []string) error {
			c := console()
			fails := 0
			ok := func(msg string) { c.Printf("%s %s\n", c.Green("✔"), msg) }
			warn := func(msg string) { c.Printf("%s %s\n", c.Yellow("!"), msg) }
			fail := func(msg string) { fails++; c.Printf("%s %s\n", c.Red("✗"), msg) }

			ok(fmt.Sprintf("tp %s on %s/%s", Version, runtime.GOOS, runtime.GOARCH))
			if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
				warn("only macOS and Linux are supported in this release")
			}
			if err := os.MkdirAll(config.AppDir(), 0o700); err != nil {
				fail("data dir " + config.AppDir() + ": " + err.Error())
			} else if f, err := os.CreateTemp(config.AppDir(), ".probe"); err != nil {
				fail("data dir not writable: " + err.Error())
			} else {
				f.Close()
				os.Remove(f.Name())
				ok("data dir " + config.AppDir())
			}
			if _, err := exec.LookPath("git"); err != nil {
				warn("git not found — repository features disabled")
			}
			p, err := agent.LoadProject(cwd())
			if p == nil {
				fail(err.Error())
				return errors.New("doctor found problems")
			}
			if err != nil {
				fail(err.Error())
			} else {
				ok("configuration valid" + c.Dim(" ("+p.Cfg.GlobalPath+existsNote(p.Cfg.GlobalPath)+")"))
			}
			for _, w := range p.Cfg.Warnings {
				warn(w)
			}
			if p.WS.IsGit {
				ok(fmt.Sprintf("git repository %s (%s)", p.WS.Root, p.WS.Branch()))
			} else {
				warn("not a git repository: " + p.WS.Root + " (diff review and git tools limited)")
			}
			if in := p.WS.Instructions(p.Cfg.Context.InstructionFiles); len(in) > 0 {
				var names []string
				for _, i := range in {
					names = append(names, i.Path)
				}
				ok("project instructions: " + strings.Join(names, ", "))
			} else {
				warn("no AGENTS.md — add one with build/test commands and conventions")
			}
			test := p.Cfg.TestCommand
			if test == "" {
				test = workspace.DetectTestCommand(p.WS.Root)
			}
			if test != "" {
				ok("test command: " + test)
			} else {
				warn("no test command detected — set test_command in .theepori/config.yaml")
			}
			if err == nil {
				ok("permission mode: " + p.Cfg.Permissions.Mode)
			}

			def := p.Cfg.ResolveDefaultModel()
			if def == "" {
				fail("no usable model: export ANTHROPIC_API_KEY or OPENAI_API_KEY, or run `tp providers discover --save` for Thinai")
			} else if _, err := providers.Target(p.Cfg, def); err != nil {
				fail("default model " + def + ": " + err.Error())
			} else {
				ok("default model: " + def)
			}
			for _, ref := range p.Cfg.FallbackModels {
				if _, err := providers.Target(p.Cfg, ref); err != nil {
					warn("fallback " + ref + ": " + err.Error())
				}
			}
			for _, name := range p.Cfg.UsableNames() {
				pr, _ := p.Cfg.Provider(name)
				usable := (!pr.NeedsKey() || pr.APIKey() != "") && !((pr.Type == "thinai" || pr.Type == "ollama") && pr.BaseURL == "")
				if !usable {
					c.Printf("%s %s\n", c.Dim("-"), c.Dim("provider "+name+": not configured (skipped)"))
					continue
				}
				if offline {
					ok("provider " + name + ": credentials present")
					continue
				}
				prov, err := providers.Build(p.Cfg, name)
				if err == nil {
					ctx, cancel := context.WithTimeout(cmd.Context(), 15*time.Second)
					var ms []llm.ModelInfo
					ms, err = prov.ListModels(ctx)
					cancel()
					if err == nil {
						ok(fmt.Sprintf("provider %s: reachable, %d models", name, len(ms)))
						continue
					}
				}
				fail("provider " + name + ": " + err.Error())
			}
			if fails > 0 {
				return fmt.Errorf("doctor found %d problem(s)", fails)
			}
			return nil
		},
	}
	cmd.Flags().BoolVar(&offline, "offline", false, "skip network checks")
	return cmd
}

// ------------------------------------------------------------------ version

func versionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print version information",
		Run: func(*cobra.Command, []string) {
			fmt.Printf("tp %s\ncommit  %s\nbuilt   %s\ngo      %s\nos/arch %s/%s\n", Version, Commit, Date, runtime.Version(), runtime.GOOS, runtime.GOARCH)
		},
	}
}

// ------------------------------------------------------------------- memory

func memoryCmd() *cobra.Command {
	store := func() (*session.Store, error) {
		p, err := agent.LoadProject(cwd())
		if p == nil {
			return nil, err
		}
		return p.Store, nil
	}
	cmd := &cobra.Command{
		Use:   "memory",
		Short: "Project memory: decisions and preferences included in every new session",
		RunE: func(*cobra.Command, []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			m := strings.TrimSpace(st.Memory())
			if m == "" {
				fmt.Println("(empty) — add with: tp memory add \"use pnpm, not npm\"")
				return nil
			}
			fmt.Println(m)
			return nil
		},
	}
	cmd.AddCommand(
		&cobra.Command{Use: "add <note>", Short: "Add a note", Args: cobra.MinimumNArgs(1), RunE: func(_ *cobra.Command, args []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			return st.Remember(strings.Join(args, " "))
		}},
		&cobra.Command{Use: "clear", Short: "Delete project memory", RunE: func(*cobra.Command, []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			return st.ClearMemory()
		}},
		&cobra.Command{Use: "path", Short: "Print the memory file path", RunE: func(*cobra.Command, []string) error {
			st, err := store()
			if err != nil {
				return err
			}
			fmt.Println(st.MemoryPath())
			return nil
		}},
	)
	return cmd
}

// --------------------------------------------------------------------- data

func dataCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "data",
		Short: "Inspect or delete everything tp stores locally",
		RunE: func(*cobra.Command, []string) error {
			c := console()
			c.Printf("data dir: %s\n", config.AppDir())
			ps, err := session.Projects()
			if err != nil {
				return err
			}
			for _, p := range ps {
				c.Printf("  %-40s %3d sessions  %s\n", p.ID, p.Sessions, c.Dim(p.Root))
			}
			if len(ps) == 0 {
				c.Printf("  %s\n", c.Dim("(no project data yet)"))
			}
			return nil
		},
	}
	var yes bool
	purge := &cobra.Command{
		Use:   "purge",
		Short: "Delete this project's sessions, memory and audit log",
		RunE: func(*cobra.Command, []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			if !yes {
				fmt.Printf("This permanently deletes %s\nType 'yes' to confirm: ", p.Store.Dir)
				line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
				if strings.TrimSpace(line) != "yes" {
					return errors.New("aborted")
				}
			}
			if err := os.RemoveAll(p.Store.Dir); err != nil {
				return err
			}
			fmt.Println("deleted", p.Store.Dir)
			return nil
		},
	}
	purge.Flags().BoolVar(&yes, "yes", false, "skip confirmation")
	cmd.AddCommand(purge)
	return cmd
}

// -------------------------------------------------------------------- audit

func auditCmd() *cobra.Command {
	var n int
	cmd := &cobra.Command{
		Use:   "audit",
		Short: "Show the local audit log of tool actions in this project",
		RunE: func(*cobra.Command, []string) error {
			p, err := agent.LoadProject(cwd())
			if p == nil {
				return err
			}
			b, err := os.ReadFile(p.Store.AuditPath())
			if errors.Is(err, os.ErrNotExist) {
				fmt.Println("no audit entries yet")
				return nil
			}
			if err != nil {
				return err
			}
			lines := strings.Split(strings.TrimSpace(string(b)), "\n")
			if len(lines) > n {
				lines = lines[len(lines)-n:]
			}
			c := console()
			for _, l := range lines {
				var e permission.AuditEntry
				if json.Unmarshal([]byte(l), &e) != nil {
					continue
				}
				dec := e.Decision
				switch dec {
				case "denied", "policy", "invalid":
					dec = c.Red(dec)
				case "approved", "always":
					dec = c.Yellow(dec)
				default:
					dec = c.Dim(dec)
				}
				extra := ""
				if e.ExitCode != nil {
					extra = fmt.Sprintf(" exit=%d", *e.ExitCode)
				}
				c.Printf("%s %-11s %-8s %-18s %s%s\n", c.Dim(e.Time.Format("01-02 15:04:05")), e.Tool, e.Risk, dec, clipLine(e.Summary, 80), c.Dim(extra))
			}
			c.Printf("%s\n", c.Dim(p.Store.AuditPath()))
			return nil
		},
	}
	cmd.Flags().IntVarP(&n, "lines", "n", 30, "number of entries to show")
	return cmd
}

func mustPreset(name string) config.Preset {
	p, _ := config.LookupPreset(name)
	return p
}

func orDefault(s string) string {
	if s == "" {
		return "(default endpoint)"
	}
	return s
}
