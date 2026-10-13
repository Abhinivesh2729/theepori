package ollama

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"sort"
	"sync"
	"time"
)

// Server is a reachable Thinai (or Ollama) endpoint.
type Server struct {
	BaseURL string
	Thinai  bool
}

// Discover finds Thinai/Ollama servers: $THINAI_HOST, this machine, then the
// /24 around each local IPv4 address. GET / answers "Thinai is running" on the
// app ("Ollama is running" on Ollama).
func Discover(ctx context.Context, port int, timeout time.Duration) []Server {
	if port == 0 {
		port = DefaultPort
	}
	if timeout == 0 {
		timeout = 700 * time.Millisecond
	}
	var hosts []string
	if h := os.Getenv("THINAI_HOST"); h != "" {
		hosts = append(hosts, NormalizeBaseURL(h))
	}
	hosts = append(hosts, fmt.Sprintf("http://127.0.0.1:%d", port))
	for _, ip := range subnetHosts() {
		hosts = append(hosts, fmt.Sprintf("http://%s:%d", ip, port))
	}

	hc := &http.Client{Timeout: timeout}
	sem := make(chan struct{}, 64)
	var mu sync.Mutex
	seen := map[string]bool{}
	var out []Server
	var wg sync.WaitGroup
	for _, h := range hosts {
		if seen[h] {
			continue
		}
		seen[h] = true
		wg.Add(1)
		go func(base string) {
			defer wg.Done()
			select {
			case sem <- struct{}{}:
			case <-ctx.Done():
				return
			}
			defer func() { <-sem }()
			if s, ok := Probe(ctx, hc, base); ok {
				mu.Lock()
				out = append(out, s)
				mu.Unlock()
			}
		}(h)
	}
	wg.Wait()
	sort.Slice(out, func(i, j int) bool {
		if out[i].Thinai != out[j].Thinai {
			return out[i].Thinai
		}
		return out[i].BaseURL < out[j].BaseURL
	})
	return out
}

// Probe checks one base URL.
func Probe(ctx context.Context, hc *http.Client, base string) (Server, bool) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, base+"/", nil)
	if err != nil {
		return Server{}, false
	}
	resp, err := hc.Do(req)
	if err != nil {
		return Server{}, false
	}
	defer resp.Body.Close()
	head := make([]byte, 16)
	n, _ := io.ReadAtLeast(resp.Body, head, 6)
	s := string(head[:n])
	switch {
	case len(s) >= 6 && s[:6] == "Thinai":
		return Server{BaseURL: base, Thinai: true}, true
	case len(s) >= 6 && s[:6] == "Ollama":
		return Server{BaseURL: base}, true
	}
	return Server{}, false
}

func subnetHosts() []string {
	var out []string
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return nil
	}
	for _, a := range addrs {
		ipn, ok := a.(*net.IPNet)
		if !ok || ipn.IP.IsLoopback() {
			continue
		}
		ip4 := ipn.IP.To4()
		if ip4 == nil || !ip4.IsPrivate() {
			continue
		}
		for i := 1; i < 255; i++ {
			out = append(out, fmt.Sprintf("%d.%d.%d.%d", ip4[0], ip4[1], ip4[2], i))
		}
	}
	return out
}
