package llm

import "strings"

// Built-in model knowledge. Prices are USD per 1M tokens and are estimates;
// override per model in config. `tp models --remote` lists live models.
var catalog = map[string][]ModelInfo{
	"anthropic": {
		{ID: "claude-opus-5-5", ContextWindow: 1_000_000, MaxOutput: 128_000, InputPrice: 4, OutputPrice: 20, CacheRead: 0.20, Tools: ToolsNative},
		{ID: "claude-sonnet-5-5", ContextWindow: 1_000_000, MaxOutput: 128_000, InputPrice: 2, OutputPrice: 10, CacheRead: 0.20, Tools: ToolsNative},
		{ID: "claude-haiku-5-5", ContextWindow: 1_000_000, MaxOutput: 128_000, InputPrice: 0.10, OutputPrice: 0.50, CacheRead: 0.01, Tools: ToolsNative},
		{ID: "claude-fable-5-1", ContextWindow: 1_000_000, MaxOutput: 128_000, InputPrice: 10, OutputPrice: 50, CacheRead: 0.25, Tools: ToolsNative},
	},
	"openai": {
		{ID: "gpt-5", ContextWindow: 400_000, MaxOutput: 128_000, InputPrice: 1.25, OutputPrice: 10, CacheRead: 0.125, Tools: ToolsNative},
		{ID: "gpt-5-mini", ContextWindow: 400_000, MaxOutput: 128_000, InputPrice: 0.25, OutputPrice: 2, CacheRead: 0.025, Tools: ToolsNative},
		{ID: "gpt-4.1", ContextWindow: 1_000_000, MaxOutput: 32_768, InputPrice: 2, OutputPrice: 8, CacheRead: 0.5, Tools: ToolsNative},
	},
}

// DefaultModel per provider type.
var defaultModel = map[string]string{
	"anthropic": "claude-opus-5-5",
	"openai":    "gpt-5",
}

func DefaultModelFor(providerType string) string { return defaultModel[providerType] }

// CatalogModels returns the built-in models for a provider type.
func CatalogModels(providerType string) []ModelInfo { return catalog[providerType] }

// LookupModel finds built-in info for a model id under a provider type,
// falling back to conservative defaults.
func LookupModel(providerType, id string) ModelInfo {
	for _, m := range catalog[providerType] {
		if m.ID == id {
			return m
		}
	}
	info := ModelInfo{ID: id, ContextWindow: 128_000, MaxOutput: 8_192, Tools: ToolsNative}
	switch providerType {
	case "thinai", "ollama":
		info.ContextWindow, info.MaxOutput, info.Tools = 8_192, 2_048, ToolsText
	case "anthropic":
		info.ContextWindow, info.MaxOutput = 200_000, 64_000
	}
	return info
}

// ParseRef splits "provider/model". Model ids may themselves contain '/'.
func ParseRef(ref string) (provider, model string) {
	provider, model, ok := strings.Cut(ref, "/")
	if !ok {
		return ref, ""
	}
	return provider, model
}

// Cost estimates USD for a usage record.
func Cost(m ModelInfo, u Usage) float64 {
	uncached := u.InputTokens
	return (float64(uncached)*m.InputPrice +
		float64(u.OutputTokens)*m.OutputPrice +
		float64(u.CacheReadTokens)*m.CacheRead +
		float64(u.CacheWriteTokens)*m.InputPrice*1.25) / 1_000_000
}
