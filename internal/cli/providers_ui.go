package cli

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/agent"
	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/providers"
	"github.com/Abhinivesh2729/theepori/internal/tui"
)

var displayName = map[string]string{
	"anthropic": "Anthropic",
	"openai":    "OpenAI",
	"ollama":    "Ollama",
	"thinai":    "Thinai",
}

// comingNext marks presets whose interactive setup isn't built yet.
var comingNext = map[string]string{
	"ollama": "coming next — local models on this machine",
	"thinai": "coming next — auto-discovers your phone",
}

func title(name string) string {
	if d, ok := displayName[name]; ok {
		return d
	}
	return name
}

// providersInteractive is `tp providers` in a terminal: pick a provider with
// the arrow keys, then add it (paste key → validated → models fetched) or
// manage it (default model, refresh, replace key, test, remove).
func providersInteractive(ctx context.Context) error {
	t := tui.New()
	cursor := 0
	for {
		p, err := agent.LoadProject(cwd())
		if p == nil {
			return err
		}
		cfg := p.Cfg
		def := cfg.ResolveDefaultModel()
		var items []tui.Item
		presets := config.Presets()
		for _, pr := range presets {
			items = append(items, providerItem(t, cfg, pr, def))
		}
		i, err := t.Select("Providers", "↑/↓ move · enter select · q quit", items, cursor)
		if errors.Is(err, tui.ErrCanceled) {
			return nil
		}
		if err != nil {
			return err
		}
		cursor = i
		pr := presets[i]
		if msg, soon := comingNext[pr.Name]; soon {
			t.Println(t.Dim("  " + title(pr.Name) + ": " + msg + "\n"))
			continue
		}
		prov, _ := cfg.Provider(pr.Name)
		if prov.KeySource() == "" {
			if err := addProviderFlow(ctx, t, cfg, pr); err != nil && !errors.Is(err, tui.ErrCanceled) {
				t.Println(t.Red("  ✗ " + err.Error()))
			}
		} else if err := manageProviderFlow(ctx, t, cfg, pr); err != nil && !errors.Is(err, tui.ErrCanceled) {
			t.Println(t.Red("  ✗ " + err.Error()))
		}
		t.Println()
	}
}

func providerItem(t *tui.Terminal, cfg *config.Loaded, pr config.Preset, def string) tui.Item {
	it := tui.Item{Label: title(pr.Name)}
	if msg, soon := comingNext[pr.Name]; soon {
		it.Detail, it.Disabled = msg, true
		return it
	}
	prov, _ := cfg.Provider(pr.Name)
	src := prov.KeySource()
	if src == "" {
		it.Detail = t.Dim("not added")
		return it
	}
	it.Detail = t.Green("✔ added") + t.Dim(" · key in "+src)
	if strings.HasPrefix(def, pr.Name+"/") {
		it.Detail += t.Dim(" · default: ") + strings.TrimPrefix(def, pr.Name+"/")
	}
	if c, ok := config.CachedModels(pr.Name); ok {
		it.Detail += t.Dim(fmt.Sprintf(" · %d models", len(c.Models)))
	}
	return it
}

// addProviderFlow: paste key → validate (one models call) → save → pick default.
func addProviderFlow(ctx context.Context, t *tui.Terminal, cfg *config.Loaded, pr config.Preset) error {
	name := pr.Name
	t.Println(t.Bold("Add " + title(name)))
	if pr.KeyURL != "" {
		t.Println(t.Dim("  Get an API key at " + pr.KeyURL))
	}
	var key string
	var models []llm.ModelInfo
	for attempt := 1; ; attempt++ {
		k, err := t.Secret("  API key: ")
		if err != nil {
			return err
		}
		if k == "" {
			return tui.ErrCanceled
		}
		fmt.Fprint(t.Out, t.Dim("  Checking key…"))
		ms, err := fetchModels(ctx, cfg, name, k)
		fmt.Fprint(t.Out, "\r\x1b[K")
		if err == nil {
			key, models = k, ms
			break
		}
		if e, ok := llm.AsError(err); ok && e.Kind == llm.ErrAuth {
			t.Println(t.Red("  ✗ "+title(name)+" rejected this key") + t.Dim(" — check it and paste again (Esc to cancel)"))
		} else {
			t.Println(t.Red("  ✗ could not verify the key: " + oneLineErr(err)))
		}
		if attempt >= 3 {
			return errors.New("key not saved")
		}
	}
	t.Println(t.Green("  ✔ key is valid") + t.Dim(fmt.Sprintf(" · %d models available", len(models))))

	store := config.Credentials()
	if err := store.Set(name, key); err != nil {
		return fmt.Errorf("could not save the key: %w", err)
	}
	if err := config.Set(config.GlobalPath(), "providers."+name+".type", pr.Type); err != nil {
		return err
	}
	_ = config.SaveModels(name, models)
	t.Println(t.Green("  ✔ saved") + t.Dim(" · key in "+store.Name()))
	if pr.APIKeyEnv != "" && pr.KeySet() {
		t.Println(t.Yellow("  ! $" + pr.APIKeyEnv + " is also exported in your shell and takes priority over the saved key"))
	}

	if err := pickDefaultModel(t, cfg, name, models); err != nil && !errors.Is(err, tui.ErrCanceled) {
		return err
	}
	t.Println(t.Green("  ✔ "+title(name)+" is ready") + t.Dim(` — try: tp "explain this repo"`))
	return nil
}

func manageProviderFlow(ctx context.Context, t *tui.Terminal, cfg *config.Loaded, pr config.Preset) error {
	name := pr.Name
	prov, _ := cfg.Provider(name)
	actions := []tui.Item{
		{Label: "Choose default model"},
		{Label: "Refresh models"},
		{Label: "Test connection"},
		{Label: "Replace API key"},
		{Label: "Remove"},
		{Label: "Back"},
	}
	i, err := t.Select(title(name), "↑/↓ move · enter select · esc back", actions, 0)
	if err != nil {
		return err
	}
	switch actions[i].Label {
	case "Choose default model":
		c, ok := config.CachedModels(name)
		models := []llm.ModelInfo(nil)
		if ok {
			models = c.Models
		} else if models, err = refreshModels(ctx, t, cfg, name); err != nil {
			return err
		}
		return pickDefaultModel(t, cfg, name, models)
	case "Refresh models":
		_, err := refreshModels(ctx, t, cfg, name)
		return err
	case "Test connection":
		testProvider(ctx, cfg, name)
	case "Replace API key":
		return addProviderFlow(ctx, t, cfg, pr)
	case "Remove":
		ok, _ := t.Confirm("Remove " + title(name) + " and delete its saved key?")
		if !ok {
			return nil
		}
		return removeProvider(t, cfg, name, prov)
	}
	return nil
}

func removeProvider(t *tui.Terminal, cfg *config.Loaded, name string, prov config.Provider) error {
	if err := config.Credentials().Delete(name); err == nil {
		t.Println(t.Green("  ✔ deleted key from " + config.Credentials().Name()))
	}
	_ = config.Unset(config.GlobalPath(), "providers."+name)
	config.DeleteCachedModels(name)
	if strings.HasPrefix(cfg.DefaultModel, name+"/") {
		_ = config.Unset(config.GlobalPath(), "default_model")
		t.Println(t.Dim("  default model cleared"))
	}
	t.Println(t.Green("  ✔ " + title(name) + " removed"))
	if prov.APIKeyEnv != "" && prov.KeySource() == "$"+prov.APIKeyEnv {
		t.Println(t.Yellow("  ! $" + prov.APIKeyEnv + " is still exported in your shell (e.g. ~/.zshrc), so " + title(name) + " stays usable until you remove it there"))
	}
	return nil
}

func refreshModels(ctx context.Context, t *tui.Terminal, cfg *config.Loaded, name string) ([]llm.ModelInfo, error) {
	fmt.Fprint(t.Out, t.Dim("  Fetching models…"))
	ms, err := fetchModels(ctx, cfg, name, "")
	fmt.Fprint(t.Out, "\r\x1b[K")
	if err != nil {
		return nil, err
	}
	_ = config.SaveModels(name, ms)
	t.Println(t.Green(fmt.Sprintf("  ✔ %d models", len(ms))))
	return ms, nil
}

// fetchModels lists models with an explicit key (or the stored one). For
// Anthropic and OpenAI this one call also proves the key is valid.
func fetchModels(ctx context.Context, cfg *config.Loaded, name, key string) ([]llm.ModelInfo, error) {
	p, err := providers.BuildWithKey(cfg, name, key)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	return p.ListModels(ctx)
}

func pickDefaultModel(t *tui.Terminal, cfg *config.Loaded, name string, models []llm.ModelInfo) error {
	if len(models) == 0 {
		return nil
	}
	current := cfg.DefaultModel // explicit default only
	suggested := mustPreset(name).DefaultModel
	start := 0
	var items []tui.Item
	for i, m := range models {
		ref := name + "/" + m.ID
		_, info, _ := cfg.Model(ref)
		detail := t.Dim(fmt.Sprintf("ctx %s", kTok(info.ContextWindow)))
		if info.InputPrice > 0 {
			detail += t.Dim(fmt.Sprintf(" · $%.2f/$%.2f per 1M", info.InputPrice, info.OutputPrice))
		}
		switch {
		case ref == current:
			detail += "  " + t.Cyan("current default")
			start = i
		case m.ID == suggested:
			detail += "  " + t.Green("recommended")
			if !strings.HasPrefix(current, name+"/") {
				start = i
			}
		}
		items = append(items, tui.Item{Label: m.ID, Detail: detail})
	}
	skip := "Skip"
	if current != "" {
		skip = "Keep current default (" + current + ")"
	}
	items = append(items, tui.Item{Label: skip})
	i, err := t.Select("Default model", "↑/↓ move · enter select · esc skip", items, start)
	if err != nil || i == len(models) {
		return err
	}
	ref := name + "/" + models[i].ID
	if err := config.Set(config.GlobalPath(), "default_model", ref); err != nil {
		return err
	}
	t.Println(t.Green("  ✔ default model: " + ref))
	return nil
}

func oneLineErr(err error) string {
	s := err.Error()
	if i := strings.Index(s, "\n"); i > 0 {
		s = s[:i]
	}
	return clipLine(s, 140)
}
