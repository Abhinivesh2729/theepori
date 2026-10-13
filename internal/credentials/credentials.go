// Package credentials stores provider API keys outside the config file:
// macOS Keychain, the Linux Secret Service (secret-tool), or a 0600 file as
// a last resort. Keys are passed to helper tools on stdin, never argv.
package credentials

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
)

const service = "theepori"

var ErrNotFound = errors.New("no stored key")

type Store interface {
	Name() string // human description, e.g. "macOS Keychain"
	Get(provider string) (string, error)
	Set(provider, key string) error
	Delete(provider string) error
}

// Open picks the best available store. TP_CREDENTIAL_STORE=keychain|secret-tool|file forces one.
func Open(appDir string) Store {
	file := &fileStore{path: filepath.Join(appDir, "credentials.json")}
	switch os.Getenv("TP_CREDENTIAL_STORE") {
	case "file":
		return cached(file)
	case "keychain":
		return cached(keychain{})
	case "secret-tool":
		return cached(secretTool{})
	}
	if runtime.GOOS == "darwin" && has("security") {
		return cached(keychain{})
	}
	if runtime.GOOS == "linux" && has("secret-tool") && os.Getenv("DBUS_SESSION_BUS_ADDRESS") != "" {
		return cached(secretTool{})
	}
	return cached(file)
}

func has(bin string) bool { _, err := exec.LookPath(bin); return err == nil }

// ---- macOS Keychain (via /usr/bin/security)

type keychain struct{}

func (keychain) Name() string { return "macOS Keychain" }

func (keychain) Get(p string) (string, error) {
	out, err := exec.Command("security", "find-generic-password", "-s", service, "-a", p, "-w").Output()
	if err != nil {
		return "", ErrNotFound
	}
	return strings.TrimRight(string(out), "\r\n"), nil
}

func (keychain) Set(p, key string) error {
	if strings.ContainsAny(key, "\"\\\n\r") {
		return errors.New("key contains characters that are not allowed")
	}
	// `security -i` reads commands from stdin, keeping the key out of argv.
	cmd := exec.Command("security", "-i")
	cmd.Stdin = strings.NewReader(fmt.Sprintf("add-generic-password -U -s %q -a %q -w \"%s\"\n", service, p, key))
	var errb bytes.Buffer
	cmd.Stderr = &errb
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("keychain: %v %s", err, strings.TrimSpace(errb.String()))
	}
	if got, err := (keychain{}).Get(p); err != nil || got != key {
		return fmt.Errorf("keychain: could not save the key %s", strings.TrimSpace(errb.String()))
	}
	return nil
}

func (keychain) Delete(p string) error {
	if err := exec.Command("security", "delete-generic-password", "-s", service, "-a", p).Run(); err != nil {
		return ErrNotFound
	}
	return nil
}

// ---- Linux Secret Service (libsecret's secret-tool)

type secretTool struct{}

func (secretTool) Name() string { return "system keyring (secret-tool)" }

func (secretTool) Get(p string) (string, error) {
	out, err := exec.Command("secret-tool", "lookup", "service", service, "account", p).Output()
	if err != nil || len(out) == 0 {
		return "", ErrNotFound
	}
	return strings.TrimRight(string(out), "\r\n"), nil
}

func (secretTool) Set(p, key string) error {
	cmd := exec.Command("secret-tool", "store", "--label=Thee Pori "+p, "service", service, "account", p)
	cmd.Stdin = strings.NewReader(key)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("secret-tool: %v %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}

func (secretTool) Delete(p string) error {
	if err := exec.Command("secret-tool", "clear", "service", service, "account", p).Run(); err != nil {
		return ErrNotFound
	}
	return nil
}

// ---- 0600 JSON file (fallback, and for tests)

type fileStore struct {
	mu   sync.Mutex
	path string
}

func (f *fileStore) Name() string { return "tp credentials file" }

func (f *fileStore) load() (map[string]string, error) {
	m := map[string]string{}
	b, err := os.ReadFile(f.path)
	if errors.Is(err, os.ErrNotExist) {
		return m, nil
	}
	if err != nil {
		return nil, err
	}
	return m, json.Unmarshal(b, &m)
}

func (f *fileStore) save(m map[string]string) error {
	if err := os.MkdirAll(filepath.Dir(f.path), 0o700); err != nil {
		return err
	}
	b, _ := json.MarshalIndent(m, "", "  ")
	tmp := f.path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, f.path)
}

func (f *fileStore) Get(p string) (string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	m, err := f.load()
	if err != nil {
		return "", err
	}
	if v, ok := m[p]; ok && v != "" {
		return v, nil
	}
	return "", ErrNotFound
}

func (f *fileStore) Set(p, key string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	m, err := f.load()
	if err != nil {
		return err
	}
	m[p] = key
	return f.save(m)
}

func (f *fileStore) Delete(p string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	m, err := f.load()
	if err != nil {
		return err
	}
	if _, ok := m[p]; !ok {
		return ErrNotFound
	}
	delete(m, p)
	return f.save(m)
}

// ---- per-process cache (each Keychain lookup spawns a process)

type cache struct {
	mu    sync.Mutex
	inner Store
	vals  map[string]string
}

func cached(s Store) Store { return &cache{inner: s, vals: map[string]string{}} }

func (c *cache) Name() string { return c.inner.Name() }

func (c *cache) Get(p string) (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if v, ok := c.vals[p]; ok {
		if v == "" {
			return "", ErrNotFound
		}
		return v, nil
	}
	v, err := c.inner.Get(p)
	c.vals[p] = v
	return v, err
}

func (c *cache) Set(p, key string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.vals, p)
	return c.inner.Set(p, key)
}

func (c *cache) Delete(p string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.vals, p)
	return c.inner.Delete(p)
}
