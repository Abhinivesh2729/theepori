// Package config loads, merges and validates global and project settings.
//
// Precedence: built-in defaults < global (~/<app dir>/config.yaml) <
// project (<repo>/.theepori/config.yaml) < flags/env. Project config is
// untrusted (it arrives with a cloned repo), so it may only tighten
// permissions and may not define providers (a base_url there could redirect
// your API keys).
package config

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"gopkg.in/yaml.v3"

	"github.com/Abhinivesh2729/theepori/internal/credentials"
	"github.com/Abhinivesh2729/theepori/internal/llm"
)

type Config struct {
	DefaultModel   string              `yaml:"default_model,omitempty"`
	FallbackModels []string            `yaml:"fallback_models,omitempty"`
	Providers      map[string]Provider `yaml:"providers,omitempty"`
	Permissions    Permissions         `yaml:"permissions,omitempty"`
	Agent          Agent               `yaml:"agent,omitempty"`
	Context        Context             `yaml:"context,omitempty"`
	TestCommand    string              `yaml:"test_command,omitempty"`
}

type Provider struct {
	Name            string                   `yaml:"-"`                // set by Loaded.Provider
	Preset          string                   `yaml:"preset,omitempty"` // fill unset fields from this preset
	Type            string                   `yaml:"type,omitempty"`
	BaseURL         string                   `yaml:"base_url,omitempty"`
	APIKeyEnv       string                   `yaml:"api_key_env,omitempty"`
	Timeout         string                   `yaml:"timeout,omitempty"`
	Headers         map[string]string        `yaml:"headers,omitempty"`
	MaxTokensField  string                   `yaml:"max_tokens_field,omitempty"`
	NumCtx          int                      `yaml:"num_ctx,omitempty"`
	RefusalFallback *bool                    `yaml:"refusal_fallback,omitempty"`
	Models          map[string]ModelOverride `yaml:"models,omitempty"`
}

type ModelOverride struct {
	ContextWindow int      `yaml:"context_window,omitempty"`
	MaxOutput     int      `yaml:"max_output,omitempty"`
	InputPrice    *float64 `yaml:"input_price,omitempty"`
	OutputPrice   *float64 `yaml:"output_price,omitempty"`
	CacheRead     *float64 `yaml:"cache_read_price,omitempty"`
	Tools         string   `yaml:"tools,omitempty"` // native | text
	Effort        string   `yaml:"effort,omitempty"`
}

type Permissions struct {
	Mode          string   `yaml:"mode,omitempty"` // plan | default | accept-edits | auto
	AllowCommands []string `yaml:"allow_commands,omitempty"`
	DenyCommands  []string `yaml:"deny_commands,omitempty"`
	DenyPaths     []string `yaml:"deny_paths,omitempty"`
}

type Agent struct {
	MaxSteps   int     `yaml:"max_steps,omitempty"`
	MaxTokens  int     `yaml:"max_tokens,omitempty"`
	Effort     string  `yaml:"effort,omitempty"`
	MaxRetries int     `yaml:"max_retries,omitempty"`
	CompactAt  float64 `yaml:"compact_at,omitempty"` // fraction of context window
}

type Context struct {
	Ignore           []string `yaml:"ignore,omitempty"`
	InstructionFiles []string `yaml:"instruction_files,omitempty"`
	MaxFileBytes     int      `yaml:"max_file_bytes,omitempty"`
}

var (
	ProviderTypes = []string{"anthropic", "openai", "openai-compatible", "thinai", "ollama"}
	Modes         = []string{"plan", "default", "accept-edits", "auto"} // strictest first
	Efforts       = []string{"low", "medium", "high", "xhigh", "max"}
)

// Defaults returns the built-in configuration.
func Defaults() Config {
	return Config{
		Providers: map[string]Provider{
			"anthropic": {Type: "anthropic", APIKeyEnv: "ANTHROPIC_API_KEY"},
			"openai":    {Type: "openai", BaseURL: "https://api.openai.com/v1", APIKeyEnv: "OPENAI_API_KEY"},
			"thinai":    {Type: "thinai", BaseURL: os.Getenv("THINAI_HOST"), APIKeyEnv: "THINAI_API_KEY"},
		},
		Permissions: Permissions{Mode: "default"},
		Agent:       Agent{MaxSteps: 60, MaxTokens: 64_000, Effort: "medium", MaxRetries: 3, CompactAt: 0.8},
		Context: Context{
			InstructionFiles: []string{"AGENTS.md", "CLAUDE.md", ".cursorrules", ".github/copilot-instructions.md"},
			MaxFileBytes:     256 * 1024,
		},
	}
}

// Loaded is the effective config plus where it came from.
type Loaded struct {
	Config
	GlobalPath  string
	ProjectPath string
	Warnings    []string
}

// AppDir is the per-user data/config directory ($TP_HOME overrides).
func AppDir() string {
	if d := os.Getenv("TP_HOME"); d != "" {
		return d
	}
	if d, err := os.UserConfigDir(); err == nil {
		return filepath.Join(d, "theepori")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".theepori")
}

func GlobalPath() string { return filepath.Join(AppDir(), "config.yaml") }

func ProjectPath(root string) string { return filepath.Join(root, ".theepori", "config.yaml") }

// Load builds the effective config for a project root ("" = none).
func Load(projectRoot string) (*Loaded, error) {
	l := &Loaded{Config: Defaults(), GlobalPath: GlobalPath()}
	if g, err := readFile(l.GlobalPath); err != nil {
		return nil, err
	} else if g != nil {
		mergeGlobal(&l.Config, g)
	}
	if projectRoot != "" {
		pp := ProjectPath(projectRoot)
		p, err := readFile(pp)
		if err != nil {
			return nil, err
		}
		if p != nil {
			l.ProjectPath = pp
			l.Warnings = append(l.Warnings, mergeProject(&l.Config, p)...)
		}
	}
	if m := os.Getenv("TP_MODEL"); m != "" {
		l.DefaultModel = m
	}
	if err := l.Validate(); err != nil {
		return l, err
	}
	return l, nil
}

func readFile(path string) (*Config, error) {
	b, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read %s: %w", path, err)
	}
	var c Config
	dec := yaml.NewDecoder(bytes.NewReader(b))
	dec.KnownFields(true)
	if err := dec.Decode(&c); err != nil && !errors.Is(err, io.EOF) {
		return nil, fmt.Errorf("invalid config %s: %v\n  → fix the file or run `tp config edit`", path, err)
	}
	return &c, nil
}

func mergeGlobal(dst *Config, src *Config) {
	if src.DefaultModel != "" {
		dst.DefaultModel = src.DefaultModel
	}
	if src.FallbackModels != nil {
		dst.FallbackModels = src.FallbackModels
	}
	for name, p := range src.Providers {
		if base, ok := dst.Providers[name]; ok {
			p = mergeProvider(base, p)
		}
		dst.Providers[name] = p
	}
	if src.Permissions.Mode != "" {
		dst.Permissions.Mode = src.Permissions.Mode
	}
	dst.Permissions.AllowCommands = append(dst.Permissions.AllowCommands, src.Permissions.AllowCommands...)
	dst.Permissions.DenyCommands = append(dst.Permissions.DenyCommands, src.Permissions.DenyCommands...)
	dst.Permissions.DenyPaths = append(dst.Permissions.DenyPaths, src.Permissions.DenyPaths...)
	mergeAgent(&dst.Agent, src.Agent)
	mergeContext(&dst.Context, src.Context)
	if src.TestCommand != "" {
		dst.TestCommand = src.TestCommand
	}
}

func mergeProvider(base, p Provider) Provider {
	if p.Type == "" {
		p.Type = base.Type
	}
	if p.BaseURL == "" {
		p.BaseURL = base.BaseURL
	}
	if p.APIKeyEnv == "" {
		p.APIKeyEnv = base.APIKeyEnv
	}
	return p
}

func mergeAgent(dst *Agent, src Agent) {
	if src.MaxSteps > 0 {
		dst.MaxSteps = src.MaxSteps
	}
	if src.MaxTokens > 0 {
		dst.MaxTokens = src.MaxTokens
	}
	if src.Effort != "" {
		dst.Effort = src.Effort
	}
	if src.MaxRetries > 0 {
		dst.MaxRetries = src.MaxRetries
	}
	if src.CompactAt > 0 {
		dst.CompactAt = src.CompactAt
	}
}

func mergeContext(dst *Context, src Context) {
	dst.Ignore = append(dst.Ignore, src.Ignore...)
	if src.InstructionFiles != nil {
		dst.InstructionFiles = src.InstructionFiles
	}
	if src.MaxFileBytes > 0 {
		dst.MaxFileBytes = src.MaxFileBytes
	}
}

// mergeProject applies untrusted project settings; returns warnings for
// anything ignored.
func mergeProject(dst *Config, src *Config) []string {
	var warn []string
	if len(src.Providers) > 0 {
		warn = append(warn, "project config: `providers` ignored — define providers in your global config (a repo must not redirect API keys)")
	}
	if len(src.Permissions.AllowCommands) > 0 {
		warn = append(warn, "project config: `permissions.allow_commands` ignored — project config can only tighten permissions")
	}
	if src.Permissions.Mode != "" {
		if slices.Index(Modes, src.Permissions.Mode) <= slices.Index(Modes, dst.Permissions.Mode) {
			dst.Permissions.Mode = src.Permissions.Mode
		} else {
			warn = append(warn, fmt.Sprintf("project config: permissions.mode %q ignored — looser than your global %q", src.Permissions.Mode, dst.Permissions.Mode))
		}
	}
	dst.Permissions.DenyCommands = append(dst.Permissions.DenyCommands, src.Permissions.DenyCommands...)
	dst.Permissions.DenyPaths = append(dst.Permissions.DenyPaths, src.Permissions.DenyPaths...)
	if src.DefaultModel != "" {
		dst.DefaultModel = src.DefaultModel
	}
	if src.FallbackModels != nil {
		dst.FallbackModels = src.FallbackModels
	}
	mergeAgent(&dst.Agent, src.Agent)
	mergeContext(&dst.Context, src.Context)
	if src.TestCommand != "" {
		dst.TestCommand = src.TestCommand
	}
	return warn
}

// Validate reports every problem at once, each with a fix.
func (l *Loaded) Validate() error {
	var errs []string
	for name := range l.Providers {
		p, _ := l.Provider(name)
		if raw := l.Providers[name]; raw.Preset != "" {
			if _, ok := LookupPreset(raw.Preset); !ok {
				errs = append(errs, fmt.Sprintf("providers.%s.preset %q is unknown (tp providers presets)", name, raw.Preset))
				continue
			}
		}
		if !slices.Contains(ProviderTypes, p.Type) {
			errs = append(errs, fmt.Sprintf("providers.%s.type %q is invalid (one of: %s)", name, p.Type, strings.Join(ProviderTypes, ", ")))
		}
		if p.Type == "openai-compatible" && p.BaseURL == "" {
			errs = append(errs, fmt.Sprintf("providers.%s.base_url is required for openai-compatible providers", name))
		}
		if p.Timeout != "" {
			if _, err := time.ParseDuration(p.Timeout); err != nil {
				errs = append(errs, fmt.Sprintf("providers.%s.timeout %q is not a duration (e.g. 90s, 5m)", name, p.Timeout))
			}
		}
		for id, m := range p.Models {
			if m.Tools != "" && m.Tools != "native" && m.Tools != "text" {
				errs = append(errs, fmt.Sprintf("providers.%s.models.%s.tools must be native or text", name, id))
			}
			if m.Effort != "" && !slices.Contains(Efforts, m.Effort) {
				errs = append(errs, fmt.Sprintf("providers.%s.models.%s.effort must be one of %s", name, id, strings.Join(Efforts, ", ")))
			}
		}
	}
	for _, ref := range append([]string{l.DefaultModel}, l.FallbackModels...) {
		if ref == "" {
			continue
		}
		prov, model := llm.ParseRef(ref)
		if _, ok := l.Provider(prov); !ok || model == "" {
			errs = append(errs, fmt.Sprintf("model %q must be <provider>/<model> with a configured provider (configured: %s; presets: tp providers presets)", ref, strings.Join(l.ProviderNames(), ", ")))
		}
	}
	if !slices.Contains(Modes, l.Permissions.Mode) {
		errs = append(errs, fmt.Sprintf("permissions.mode %q is invalid (one of: %s)", l.Permissions.Mode, strings.Join(Modes, ", ")))
	}
	if l.Agent.Effort != "" && !slices.Contains(Efforts, l.Agent.Effort) {
		errs = append(errs, fmt.Sprintf("agent.effort %q is invalid (one of: %s)", l.Agent.Effort, strings.Join(Efforts, ", ")))
	}
	if len(errs) > 0 {
		slices.Sort(errs)
		return fmt.Errorf("invalid configuration:\n  - %s\n  → edit with `tp config edit` (global) or .theepori/config.yaml (project)", strings.Join(errs, "\n  - "))
	}
	return nil
}

func (l *Loaded) ProviderNames() []string {
	var names []string
	for n := range l.Providers {
		names = append(names, n)
	}
	slices.Sort(names)
	return names
}

// APIKey resolves a provider's key: its env var first, then the credential
// store (Keychain etc.) where `tp providers` saves pasted keys.
func (p Provider) APIKey() string {
	if p.APIKeyEnv != "" {
		if v := strings.TrimSpace(os.Getenv(p.APIKeyEnv)); v != "" {
			return v
		}
	}
	if p.Name != "" && p.NeedsKey() {
		if v, err := Credentials().Get(p.Name); err == nil {
			return v
		}
	}
	return ""
}

// KeySource says where APIKey came from ("" when there is no key).
func (p Provider) KeySource() string {
	if p.APIKeyEnv != "" && strings.TrimSpace(os.Getenv(p.APIKeyEnv)) != "" {
		return "$" + p.APIKeyEnv
	}
	if p.Name != "" && p.NeedsKey() {
		if _, err := Credentials().Get(p.Name); err == nil {
			return Credentials().Name()
		}
	}
	return ""
}

var (
	credMu    sync.Mutex
	credStore credentials.Store
	credKey   string
)

// Credentials returns the key store for the current app dir.
func Credentials() credentials.Store {
	credMu.Lock()
	defer credMu.Unlock()
	k := AppDir() + "|" + os.Getenv("TP_CREDENTIAL_STORE")
	if credStore == nil || credKey != k {
		credStore, credKey = credentials.Open(AppDir()), k
	}
	return credStore
}

// NeedsKey reports whether the provider requires an API key. Compatible
// endpoints need one when a key variable is configured (local servers don't).
func (p Provider) NeedsKey() bool {
	return p.Type == "anthropic" || p.Type == "openai" || (p.Type == "openai-compatible" && p.APIKeyEnv != "")
}

func (p Provider) TimeoutDuration() time.Duration {
	d, _ := time.ParseDuration(p.Timeout)
	return d
}

// ResolveDefaultModel picks the configured default, else the first provider
// with credentials available.
func (l *Loaded) ResolveDefaultModel() string {
	if l.DefaultModel != "" {
		return l.DefaultModel
	}
	for _, name := range []string{"anthropic", "openai"} {
		if p, ok := l.Provider(name); ok && p.APIKey() != "" {
			return name + "/" + llm.DefaultModelFor(p.Type)
		}
	}
	if p, ok := l.Provider("thinai"); ok && p.BaseURL != "" {
		return "thinai/loaded"
	}
	return ""
}

// Model resolves catalog info plus config overrides for "provider/model".
func (l *Loaded) Model(ref string) (Provider, llm.ModelInfo, error) {
	prov, id := llm.ParseRef(ref)
	p, ok := l.Provider(prov)
	if !ok || id == "" {
		return Provider{}, llm.ModelInfo{}, fmt.Errorf("unknown model %q — use <provider>/<model>; providers: %s", ref, strings.Join(l.ProviderNames(), ", "))
	}
	ctype := p.Type
	if ctype == "openai-compatible" {
		ctype = "openai"
	}
	info := llm.LookupModel(ctype, id)
	if p.Type == "openai-compatible" && info.InputPrice == 0 {
		info.Tools = llm.ToolsNative
	}
	if o, ok := p.Models[id]; ok {
		if o.ContextWindow > 0 {
			info.ContextWindow = o.ContextWindow
		}
		if o.MaxOutput > 0 {
			info.MaxOutput = o.MaxOutput
		}
		if o.InputPrice != nil {
			info.InputPrice = *o.InputPrice
		}
		if o.OutputPrice != nil {
			info.OutputPrice = *o.OutputPrice
		}
		if o.CacheRead != nil {
			info.CacheRead = *o.CacheRead
		}
		if o.Tools != "" {
			info.Tools = llm.ToolMode(o.Tools)
		}
	}
	if p.NumCtx > 0 && (p.Type == "thinai" || p.Type == "ollama") {
		info.ContextWindow = p.NumCtx
	}
	return p, info, nil
}

// EffortFor returns the effort for a model (per-model override, else agent default).
func (l *Loaded) EffortFor(ref string) string {
	prov, id := llm.ParseRef(ref)
	p, _ := l.Provider(prov)
	if o, ok := p.Models[id]; ok && o.Effort != "" {
		return o.Effort
	}
	return l.Agent.Effort
}
