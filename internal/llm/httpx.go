package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"time"
)

// HTTPClient builds a client suited to streaming: no overall deadline (long
// generations are fine) but bounded connect and time-to-first-byte.
func HTTPClient(timeout time.Duration) *http.Client {
	if timeout <= 0 {
		timeout = 120 * time.Second
	}
	return &http.Client{Transport: &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           (&net.Dialer{Timeout: 10 * time.Second}).DialContext,
		ResponseHeaderTimeout: timeout,
		TLSHandshakeTimeout:   10 * time.Second,
		MaxIdleConnsPerHost:   4,
	}}
}

// PostJSON sends body and returns the response if status is 2xx; otherwise a
// classified *Error. The caller closes the body.
func PostJSON(ctx context.Context, hc *http.Client, provider, url string, headers map[string]string, body any) (*http.Response, error) {
	buf, err := json.Marshal(body)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(buf))
	if err != nil {
		return nil, &Error{Provider: provider, Kind: ErrConfig, Message: err.Error(), Hint: "check base_url"}
	}
	req.Header.Set("Content-Type", "application/json")
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	return do(hc, req, provider)
}

// GetJSON fetches url and decodes the JSON response into out.
func GetJSON(ctx context.Context, hc *http.Client, provider, url string, headers map[string]string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return &Error{Provider: provider, Kind: ErrConfig, Message: err.Error(), Hint: "check base_url"}
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := do(hc, req, provider)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if err := json.NewDecoder(resp.Body).Decode(out); err != nil {
		return &Error{Provider: provider, Kind: ErrServer, Message: "invalid JSON from " + url + ": " + err.Error()}
	}
	return nil
}

func do(hc *http.Client, req *http.Request, provider string) (*http.Response, error) {
	resp, err := hc.Do(req)
	if err != nil {
		return nil, FromTransport(provider, req.URL.Scheme+"://"+req.URL.Host, err)
	}
	if resp.StatusCode/100 != 2 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 8192))
		resp.Body.Close()
		return nil, FromStatus(provider, resp.StatusCode, string(b), resp.Header)
	}
	return resp, nil
}
