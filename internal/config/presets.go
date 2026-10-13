package config

import (
	"os"
	"sort"
	"strings"
)

// Preset is a known provider: everything needed to use it except the key.
type Preset struct {
	Name         string
	Type         string
	BaseURL      string
	APIKeyEnv    string
	DefaultModel string // suggestion only; providers rename models often
	KeyURL       string // where to get a key
	Local        bool   // runs on this machine / LAN, no key
}

// Presets are usable without configuration: `tp -m groq/<model>` works as
// soon as GROQ_API_KEY is set. `tp providers add <preset>` saves one, and
// `--preset` lets several entries share a preset (e.g. work vs personal key).
var presets = []Preset{
	{Name: "anthropic", Type: "anthropic", APIKeyEnv: "ANTHROPIC_API_KEY", DefaultModel: "claude-opus-5-5", KeyURL: "https://console.anthropic.com/settings/keys"},
	{Name: "openai", Type: "openai", BaseURL: "https://api.openai.com/v1", APIKeyEnv: "OPENAI_API_KEY", DefaultModel: "gpt-5", KeyURL: "https://platform.openai.com/api-keys"},
	{Name: "gemini", Type: "openai-compatible", BaseURL: "https://generativelanguage.googleapis.com/v1beta/openai", APIKeyEnv: "GEMINI_API_KEY", KeyURL: "https://aistudio.google.com/apikey"},
	{Name: "groq", Type: "openai-compatible", BaseURL: "https://api.groq.com/openai/v1", APIKeyEnv: "GROQ_API_KEY", DefaultModel: "llama-3.3-70b-versatile", KeyURL: "https://console.groq.com/keys"},
	{Name: "openrouter", Type: "openai-compatible", BaseURL: "https://openrouter.ai/api/v1", APIKeyEnv: "OPENROUTER_API_KEY", KeyURL: "https://openrouter.ai/keys"},
	{Name: "deepseek", Type: "openai-compatible", BaseURL: "https://api.deepseek.com/v1", APIKeyEnv: "DEEPSEEK_API_KEY", DefaultModel: "deepseek-chat", KeyURL: "https://platform.deepseek.com/api_keys"},
	{Name: "mistral", Type: "openai-compatible", BaseURL: "https://api.mistral.ai/v1", APIKeyEnv: "MISTRAL_API_KEY", DefaultModel: "mistral-large-latest", KeyURL: "https://console.mistral.ai/api-keys"},
	{Name: "xai", Type: "openai-compatible", BaseURL: "https://api.x.ai/v1", APIKeyEnv: "XAI_API_KEY", KeyURL: "https://console.x.ai"},
	{Name: "together", Type: "openai-compatible", BaseURL: "https://api.together.xyz/v1", APIKeyEnv: "TOGETHER_API_KEY", KeyURL: "https://api.together.ai/settings/api-keys"},
	{Name: "fireworks", Type: "openai-compatible", BaseURL: "https://api.fireworks.ai/inference/v1", APIKeyEnv: "FIREWORKS_API_KEY", KeyURL: "https://fireworks.ai/account/api-keys"},
	{Name: "cerebras", Type: "openai-compatible", BaseURL: "https://api.cerebras.ai/v1", APIKeyEnv: "CEREBRAS_API_KEY", KeyURL: "https://cloud.cerebras.ai"},
	{Name: "thinai", Type: "thinai", APIKeyEnv: "THINAI_API_KEY", DefaultModel: "loaded", Local: true},
	{Name: "ollama", Type: "ollama", BaseURL: "http://localhost:11434", DefaultModel: "loaded", Local: true},
	{Name: "lmstudio", Type: "openai-compatible", BaseURL: "http://localhost:1234/v1", Local: true},
}

// Presets returns all presets sorted by name.
func Presets() []Preset {
	out := append([]Preset(nil), presets...)
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

func LookupPreset(name string) (Preset, bool) {
	for _, p := range presets {
		if p.Name == name {
			return p, true
		}
	}
	return Preset{}, false
}

// KeySet reports whether the preset's key is present in the environment.
func (p Preset) KeySet() bool {
	return p.APIKeyEnv != "" && strings.TrimSpace(os.Getenv(p.APIKeyEnv)) != ""
}

// fill completes a provider entry from a preset; explicit fields win.
func (p Provider) fill(pr Preset) Provider {
	if p.Type == "" {
		p.Type = pr.Type
	}
	if p.BaseURL == "" {
		p.BaseURL = pr.BaseURL
	}
	if p.APIKeyEnv == "" {
		p.APIKeyEnv = pr.APIKeyEnv
	}
	return p
}

// Provider resolves a provider by name: configured entries (completed from
// their preset), else an unconfigured preset of that name.
func (l *Loaded) Provider(name string) (Provider, bool) {
	p, configured := l.Providers[name]
	presetName := p.Preset
	if presetName == "" {
		presetName = name
	}
	if pr, ok := LookupPreset(presetName); ok {
		return p.fill(pr), true
	}
	return p, configured
}

// UsableNames lists configured providers plus remote presets whose key is
// present in the environment.
func (l *Loaded) UsableNames() []string {
	names := l.ProviderNames()
	seen := map[string]bool{}
	for _, n := range names {
		seen[n] = true
	}
	for _, pr := range presets {
		if !seen[pr.Name] && !pr.Local && pr.KeySet() {
			names = append(names, pr.Name)
		}
	}
	sort.Strings(names)
	return names
}
