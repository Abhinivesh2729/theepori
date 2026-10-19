// genfixtures writes Go-produced fixtures for the TP Vault Rust port.
// Usage: copy into panuval-cli as cmd/genfixtures/main.go (it imports
// internal packages), then: go run ./cmd/genfixtures <outdir>
// and copy go_vectors.json and panuval_store/cache/*.json into this dir.
package main

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/Abhinivesh2729/panuval-cli/internal/crypto"
	"github.com/Abhinivesh2729/panuval-cli/internal/store"
	"github.com/Abhinivesh2729/panuval-cli/internal/vault"
)

type openVec struct {
	Name         string `json:"name"`
	KeyHex       string `json:"keyHex"`
	AADHex       string `json:"aadHex"`
	PlaintextHex string `json:"plaintextHex"`
	BlobHex      string `json:"blobHex"`
}

type chainItem struct {
	ItemID       string `json:"itemId"`
	Version      int64  `json:"version"`
	PlaintextHex string `json:"plaintextHex"`
	BlobHex      string `json:"blobHex"`
}

type chain struct {
	Password    string      `json:"password"`
	UID         string      `json:"uid"`
	SaltHex     string      `json:"saltHex"`
	MemoryKiB   uint32      `json:"memoryKiB"`
	Iterations  uint32      `json:"iterations"`
	Parallelism uint8       `json:"parallelism"`
	KEKHex      string      `json:"kekHex"`
	DEKHex      string      `json:"dekHex"`
	WrappedHex  string      `json:"wrappedDekHex"`
	Items       []chainItem `json:"items"`
}

type out struct {
	Generator string    `json:"generator"`
	Open      []openVec `json:"open"`
	Chain     chain     `json:"chain"`
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func check(err error) {
	if err != nil {
		panic(err)
	}
}

type localCipher struct{ key []byte }

func (c localCipher) Seal(pt, aad []byte) ([]byte, error) { return crypto.Seal(c.key, pt, aad) }
func (c localCipher) Open(b, aad []byte) ([]byte, error)  { return crypto.Open(c.key, b, aad) }

func main() {
	dir := os.Args[1]
	check(os.MkdirAll(dir, 0o755))

	// 1. Random-nonce blobs from crypto.Seal (what Panuval actually stores).
	key := must(crypto.NewDEK())
	var o out
	o.Generator = "panuval-cli internal/crypto via cmd/genfixtures (random nonces)"
	cases := []struct {
		name string
		pt   []byte
		aad  []byte
	}{
		{"ascii", []byte("sk-live-0123456789"), crypto.ItemAAD("uid-1", "item-1", 1)},
		{"unicode", []byte("தமிழ் café 🔑"), crypto.ItemAAD("uid-1", "item-2", 42)},
		{"empty-plaintext", []byte{}, crypto.ItemAAD("uid-1", "item-3", 1)},
		{"nil-aad", []byte("no aad"), nil},
		{"binary-4k", bytesSeq(4096), []byte("panuval:dek:v1")},
	}
	for _, c := range cases {
		blob := must(crypto.Seal(key, c.pt, c.aad))
		o.Open = append(o.Open, openVec{c.name, hex.EncodeToString(key), hex.EncodeToString(c.aad), hex.EncodeToString(c.pt), hex.EncodeToString(blob)})
	}

	// 2. A full password -> KEK -> DEK -> item chain.
	p := crypto.KDFParams{Alg: "argon2id", MemoryKiB: 2048, Iterations: 2, Parallelism: 1, Salt: must(crypto.Random(crypto.SaltSize))}
	pw := "Tamil vault ச password"
	kek := must(crypto.DeriveKEK(pw, p))
	dek := must(crypto.NewDEK())
	wrapped := must(crypto.WrapDEK(kek, dek))
	ch := chain{Password: pw, UID: "uid-chain", SaltHex: hex.EncodeToString(p.Salt), MemoryKiB: p.MemoryKiB, Iterations: p.Iterations,
		Parallelism: p.Parallelism, KEKHex: hex.EncodeToString(kek), DEKHex: hex.EncodeToString(dek), WrappedHex: hex.EncodeToString(wrapped)}
	for i, it := range []vault.Item{
		{Name: "OPENAI_API_KEY", Type: "apiKey", Value: "sk-chain-1"},
		{Name: "DB_URL", Type: "env", Folder: "projects/x", Value: "postgres://localhost/தமிழ்"},
	} {
		now := time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)
		it.CreatedAt, it.UpdatedAt = now, now
		pt := must(it.Marshal())
		id := vault.NewID()
		ver := int64(i + 1)
		blob := must(crypto.Seal(dek, pt, crypto.ItemAAD(ch.UID, id, ver)))
		ch.Items = append(ch.Items, chainItem{id, ver, hex.EncodeToString(pt), hex.EncodeToString(blob)})
	}
	o.Chain = ch
	raw := must(json.MarshalIndent(o, "", "  "))
	check(os.WriteFile(filepath.Join(dir, "go_vectors.json"), append(raw, '\n'), 0o644))

	// 3. A Panuval local store (cache file) written by the real vault code.
	storeDir := filepath.Join(dir, "panuval_store")
	cachePath := filepath.Join(storeDir, "cache", "panuval-atmega-uid-fixture.json")
	_ = os.RemoveAll(storeDir)
	const uid = "uid-fixture"
	const storePW = "panuval fixture password"
	sp := crypto.KDFParams{Alg: "argon2id", MemoryKiB: 4096, Iterations: 2, Parallelism: 1, Salt: must(crypto.Random(crypto.SaltSize))}
	skek := must(crypto.DeriveKEK(storePW, sp))
	sdek := must(crypto.NewDEK())
	swrapped := must(crypto.WrapDEK(skek, sdek))
	c := store.Cache{Path: cachePath, UID: uid}
	check(c.Update(func(d *store.CacheData) error {
		d.Meta = &store.Meta{FormatVersion: 1, WrappedDEK: swrapped, CreatedAt: time.Date(2026, 9, 1, 0, 0, 0, 0, time.UTC),
			UpdateTime: "2026-09-01T00:00:00.000000Z", KDF: store.KDF{Alg: sp.Alg, MemoryKiB: int64(sp.MemoryKiB),
				Iterations: int64(sp.Iterations), Parallelism: int64(sp.Parallelism), Salt: sp.Salt}}
		return nil
	}))
	r := &store.Remote{ProjectID: "panuval-atmega", BaseURL: "http://127.0.0.1:1", UID: uid,
		Token: func(context.Context) (string, error) { return "t", nil }}
	v := &vault.Vault{UID: uid, Cipher: localCipher{sdek}, Syncer: &store.Syncer{Remote: r, Cache: c},
		Actor: vault.Actor{Kind: "human", DeviceID: "dev-fixture", SessionID: "s1"}}
	create := func(it vault.Item) { check(v.Create(it)) }
	create(vault.Item{Name: "OPENAI_API_KEY", Folder: "projects/thinai", Type: "apiKey", Value: "sk-thinai-0001", AgentAccess: true})
	create(vault.Item{Name: "OPENAI_API_KEY", Folder: "projects/kavi", Type: "apiKey", Value: "sk-kavi-0001"})
	create(vault.Item{Name: "GITHUB_TOKEN", Type: "token", Value: "ghp_fixture_token"})
	create(vault.Item{Name: "DATABASE_URL", Folder: "projects/thinai", Type: "env", Value: "postgres://u:p@localhost/db?x=தமிழ்"})
	create(vault.Item{Name: "github", Folder: "personal", Type: "password", Value: "hunter2", Username: "me"})
	create(vault.Item{Name: "recovery", Folder: "notes", Type: "note", Value: "line one\nline two"})
	create(vault.Item{Name: "EMPTY_VALUE", Type: "env", Value: ""})
	create(vault.Item{Name: "OLD_KEY", Type: "env", Value: "old"})
	check(v.Delete(must(v.Find("OLD_KEY"))))
	cur := must(v.Find("projects/thinai/OPENAI_API_KEY"))
	next := cur
	next.Value = "sk-thinai-0002"
	check(v.Update(cur, next))
	items := must(v.List())
	var names []string
	for _, it := range items {
		names = append(names, it.Path())
	}
	println("store items:", strings.Join(names, ", "))
}

func bytesSeq(n int) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte(i * 7)
	}
	return b
}
