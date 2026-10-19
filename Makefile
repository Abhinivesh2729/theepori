# Thee Pori software factory (macOS only). See docs/FACTORY_DESIGN.md.
.PHONY: build test lint desktop app perf install-daemon clean site site-deploy

build:
	cargo build --release -p tpd -p tp-cli

test:
	cargo test --workspace

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd apps/desktop && npm run typecheck

desktop:
	sh scripts/build-app.sh

app: desktop

perf: build desktop
	python3 -I scripts/perf.py

# Installs target/release/tp to ~/.local/bin and tpd as a launchd agent.
install-daemon: build
	install -d $(HOME)/.local/bin
	install -m 0755 target/release/tp $(HOME)/.local/bin/tp
	target/release/tp daemon install --tpd target/release/tpd

clean:
	cargo clean
	rm -rf apps/desktop/dist

# Landing page + installer -> https://tp.0xabhi.space (Cloudflare Worker, static assets)
site:
	cp install.sh site/public/install.sh

site-deploy: site
	cd site && npx -y wrangler@4 deploy

