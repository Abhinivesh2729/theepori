package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

// Set writes a dotted key (e.g. "permissions.mode") into a YAML file,
// preserving comments and other keys. The value is parsed as YAML, so
// "true", "5" and "[a, b]" get their natural types.
func Set(path, key, value string) error {
	doc, err := loadNode(path)
	if err != nil {
		return err
	}
	var val yaml.Node
	if err := yaml.Unmarshal([]byte(value), &val); err != nil || len(val.Content) == 0 {
		val = yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{{Kind: yaml.ScalarNode, Value: value}}}
	}
	m := doc.Content[0]
	parts := strings.Split(key, ".")
	for i, k := range parts {
		child := lookup(m, k)
		if i == len(parts)-1 {
			if child != nil {
				*child = *val.Content[0]
			} else {
				m.Content = append(m.Content, &yaml.Node{Kind: yaml.ScalarNode, Value: k}, val.Content[0])
			}
			break
		}
		if child == nil {
			child = &yaml.Node{Kind: yaml.MappingNode}
			m.Content = append(m.Content, &yaml.Node{Kind: yaml.ScalarNode, Value: k}, child)
		}
		if child.Kind != yaml.MappingNode {
			return fmt.Errorf("%s is not a map", strings.Join(parts[:i+1], "."))
		}
		m = child
	}
	return saveNode(path, doc)
}

// Unset removes a dotted key.
func Unset(path, key string) error {
	doc, err := loadNode(path)
	if err != nil {
		return err
	}
	m := doc.Content[0]
	parts := strings.Split(key, ".")
	for i, k := range parts {
		for j := 0; j+1 < len(m.Content); j += 2 {
			if m.Content[j].Value != k {
				continue
			}
			if i == len(parts)-1 {
				m.Content = append(m.Content[:j], m.Content[j+2:]...)
				return saveNode(path, doc)
			}
			m = m.Content[j+1]
			break
		}
	}
	return fmt.Errorf("key %q not set", key)
}

func lookup(m *yaml.Node, key string) *yaml.Node {
	for j := 0; j+1 < len(m.Content); j += 2 {
		if m.Content[j].Value == key {
			return m.Content[j+1]
		}
	}
	return nil
}

func loadNode(path string) (*yaml.Node, error) {
	doc := &yaml.Node{}
	b, err := os.ReadFile(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	if len(strings.TrimSpace(string(b))) > 0 {
		if err := yaml.Unmarshal(b, doc); err != nil {
			return nil, fmt.Errorf("parse %s: %w", path, err)
		}
	}
	if doc.Kind == 0 || len(doc.Content) == 0 {
		doc = &yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{{Kind: yaml.MappingNode}}}
	}
	return doc, nil
}

func saveNode(path string, doc *yaml.Node) error {
	b, err := yaml.Marshal(doc)
	if err != nil {
		return err
	}
	// Validate before writing so a bad `set` can't brick the config.
	var c Config
	if err := yaml.Unmarshal(b, &c); err != nil {
		return fmt.Errorf("resulting config is invalid: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	return WriteFileAtomic(path, b, 0o600)
}

// WriteFileAtomic writes via a temp file + rename.
func WriteFileAtomic(path string, data []byte, perm os.FileMode) error {
	tmp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+".tmp*")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	if err := os.Chmod(tmp.Name(), perm); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return os.Rename(tmp.Name(), path)
}
