package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"time"

	"github.com/Abhinivesh2729/theepori/internal/llm"
)

// ModelCache is the model list fetched from a provider (at `providers add`
// or `models --remote`), so listings don't need a network call.
type ModelCache struct {
	FetchedAt time.Time       `json:"fetched_at"`
	Models    []llm.ModelInfo `json:"models"`
}

func modelCachePath(provider string) string {
	return filepath.Join(AppDir(), "models", provider+".json")
}

func SaveModels(provider string, models []llm.ModelInfo) error {
	p := modelCachePath(provider)
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		return err
	}
	b, err := json.MarshalIndent(ModelCache{FetchedAt: time.Now(), Models: models}, "", "  ")
	if err != nil {
		return err
	}
	return WriteFileAtomic(p, b, 0o600)
}

func CachedModels(provider string) (*ModelCache, bool) {
	b, err := os.ReadFile(modelCachePath(provider))
	if err != nil {
		return nil, false
	}
	var c ModelCache
	if json.Unmarshal(b, &c) != nil || len(c.Models) == 0 {
		return nil, false
	}
	return &c, true
}

func DeleteCachedModels(provider string) { _ = os.Remove(modelCachePath(provider)) }
