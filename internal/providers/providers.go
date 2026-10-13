// Package providers builds llm.Provider instances from configuration.
package providers

import (
	"fmt"

	"github.com/Abhinivesh2729/theepori/internal/config"
	"github.com/Abhinivesh2729/theepori/internal/llm"
	"github.com/Abhinivesh2729/theepori/internal/llm/anthropic"
	"github.com/Abhinivesh2729/theepori/internal/llm/ollama"
	"github.com/Abhinivesh2729/theepori/internal/llm/openai"
)

// Build constructs the named provider. Missing credentials are an
// actionable error, not a late HTTP 401.
func Build(cfg *config.Loaded, name string) (llm.Provider, error) {
	p, ok := cfg.Provider(name)
	if !ok {
		return nil, &llm.Error{Provider: name, Kind: llm.ErrConfig, Message: "unknown provider",
			Hint: "see `tp providers presets`, or add a custom one: tp providers add " + name + " --type openai-compatible --base-url ..."}
	}
	key := p.APIKey()
	if p.NeedsKey() && key == "" {
		hint := fmt.Sprintf("export %s=... (see `tp auth`)", p.APIKeyEnv)
		presetName := cfg.Providers[name].Preset
		if presetName == "" {
			presetName = name
		}
		if pr, ok := config.LookupPreset(presetName); ok && pr.KeyURL != "" {
			hint = fmt.Sprintf("get a key at %s, then: export %s=...", pr.KeyURL, p.APIKeyEnv)
		}
		return nil, &llm.Error{Provider: name, Kind: llm.ErrAuth, Message: fmt.Sprintf("environment variable %s is not set", p.APIKeyEnv), Hint: hint}
	}
	switch p.Type {
	case "anthropic":
		fallback := p.RefusalFallback == nil || *p.RefusalFallback
		return anthropic.New(anthropic.Config{Name: name, BaseURL: p.BaseURL, APIKey: key, Timeout: p.TimeoutDuration(), RefusalFallback: fallback}), nil
	case "openai", "openai-compatible":
		base := p.BaseURL
		if base == "" {
			base = "https://api.openai.com/v1"
		}
		return openai.New(openai.Config{Name: name, Type: p.Type, BaseURL: base, APIKey: key, Timeout: p.TimeoutDuration(),
			Headers: p.Headers, MaxTokensField: p.MaxTokensField}), nil
	case "thinai", "ollama":
		return ollama.New(ollama.Config{Name: name, Type: p.Type, BaseURL: p.BaseURL, APIKey: key, Timeout: p.TimeoutDuration(), NumCtx: p.NumCtx}), nil
	}
	return nil, &llm.Error{Provider: name, Kind: llm.ErrConfig, Message: "unknown provider type " + p.Type}
}

// Client builds a retrying client for a primary model plus configured fallbacks.
// Fallbacks whose provider cannot be built are skipped with a warning.
func Client(cfg *config.Loaded, primary string, notify func(string)) (*llm.Client, error) {
	c := &llm.Client{MaxRetries: cfg.Agent.MaxRetries, Notify: notify}
	for i, ref := range append([]string{primary}, cfg.FallbackModels...) {
		if i > 0 && ref == primary {
			continue
		}
		t, err := Target(cfg, ref)
		if err != nil {
			if i == 0 {
				return nil, err
			}
			if notify != nil {
				notify(fmt.Sprintf("fallback %s unavailable: %v", ref, err))
			}
			continue
		}
		c.Targets = append(c.Targets, t)
	}
	return c, nil
}

// Target resolves one "provider/model" reference.
func Target(cfg *config.Loaded, ref string) (llm.Target, error) {
	if ref == "" {
		return llm.Target{}, &llm.Error{Provider: "tp", Kind: llm.ErrConfig, Message: "no model selected",
			Hint: "set ANTHROPIC_API_KEY or OPENAI_API_KEY, or run `tp models use <provider/model>`"}
	}
	prov, info, err := cfg.Model(ref)
	if err != nil {
		return llm.Target{}, err
	}
	_ = prov
	name, _ := llm.ParseRef(ref)
	p, err := Build(cfg, name)
	if err != nil {
		return llm.Target{}, err
	}
	return llm.Target{Ref: ref, Provider: p, Model: info}, nil
}

// KeyEnvVars lists every configured credential env var (for redaction and
// stripping from child processes).
func KeyEnvVars(cfg *config.Loaded) []string {
	seen := map[string]bool{}
	var out []string
	add := func(env string) {
		if env != "" && !seen[env] {
			seen[env] = true
			out = append(out, env)
		}
	}
	for name := range cfg.Providers {
		p, _ := cfg.Provider(name)
		add(p.APIKeyEnv)
	}
	// Preset keys are protected too, even when unused.
	for _, pr := range config.Presets() {
		add(pr.APIKeyEnv)
	}
	return out
}
