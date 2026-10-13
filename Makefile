VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo 0.1.0-dev)
COMMIT  ?= $(shell git rev-parse --short HEAD 2>/dev/null || echo none)
DATE    ?= $(shell date -u +%Y-%m-%dT%H:%M:%SZ)
PKG     := github.com/Abhinivesh2729/theepori/internal/cli
LDFLAGS := -s -w -X $(PKG).Version=$(VERSION) -X $(PKG).Commit=$(COMMIT) -X $(PKG).Date=$(DATE)
PREFIX  ?= $(HOME)/.local

.PHONY: build test vet install dist clean site site-deploy snapshot

build:
	go build -ldflags "$(LDFLAGS)" -o bin/tp ./cmd/tp

test:
	go test ./...

vet:
	go vet ./...
	@test -z "$$(gofmt -l .)" || (gofmt -l . && echo "run gofmt -w ." && exit 1)

install: build
	install -d $(PREFIX)/bin
	install -m 0755 bin/tp $(PREFIX)/bin/tp
	@echo "installed $(PREFIX)/bin/tp"

# Cross-compile release binaries for macOS and Linux.
dist:
	@mkdir -p dist
	@for os in darwin linux; do for arch in amd64 arm64; do \
		echo "building $$os/$$arch"; \
		CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch go build -ldflags "$(LDFLAGS)" -o dist/tp-$$os-$$arch ./cmd/tp || exit 1; \
	done; done

clean:
	rm -rf bin dist

# Landing page + installer → https://tp.0xabby.space (Cloudflare Worker, static assets)
site:
	cp install.sh site/public/install.sh

site-deploy: site
	cd site && npx -y wrangler@4 deploy

# Local dry run of the release (no publish)
snapshot:
	goreleaser release --snapshot --clean
