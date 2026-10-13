#!/bin/sh
# Thee Pori (தீப்பொறி) installer: https://tp.0xabhi.space
#
#   curl -fsSL https://tp.0xabhi.space/install.sh | sh
#
# Options (environment variables):
#   TP_VERSION=v0.1.0          install a specific release (default: latest)
#   TP_INSTALL_DIR=/usr/local/bin   install location (default: ~/.local/bin)
# Uninstall:
#   curl -fsSL https://tp.0xabhi.space/install.sh | sh -s -- --uninstall
#
# The whole script is wrapped in main() so a partial download never runs.
set -eu

REPO="Abhinivesh2729/theepori"

main() {
	INSTALL_DIR="${TP_INSTALL_DIR:-$HOME/.local/bin}"

	if [ "${1:-}" = "--uninstall" ]; then
		uninstall
		return
	fi

	if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
		B="$(printf '\033[1m')" D="$(printf '\033[2m')" R="$(printf '\033[31m')" G="$(printf '\033[32m')" Y="$(printf '\033[33m')" O="$(printf '\033[38;5;208m')" N="$(printf '\033[0m')"
	else
		B="" D="" R="" G="" Y="" O="" N=""
	fi

	os="$(detect_os)"
	arch="$(detect_arch "$os")"
	asset="tp_${os}_${arch}.tar.gz"

	version="${TP_VERSION:-latest}"
	if [ -n "${TP_DOWNLOAD_BASE:-}" ]; then # mirrors and testing
		base="$TP_DOWNLOAD_BASE"
	elif [ "$version" = "latest" ]; then
		base="https://github.com/${REPO}/releases/latest/download"
	else
		case "$version" in v*) ;; *) version="v$version" ;; esac
		base="https://github.com/${REPO}/releases/download/${version}"
	fi

	printf '%s\n' "${O}${B}தீப்பொறி${N} ${B}Thee Pori${N} installer"
	printf '%s\n' "${D}  platform ${os}/${arch} · release ${version}${N}"

	tmp="$(mktemp -d 2>/dev/null || mktemp -d -t tp)"
	trap 'rm -rf "$tmp"' EXIT INT TERM

	download "$base/$asset" "$tmp/$asset"
	download "$base/checksums.txt" "$tmp/checksums.txt"
	verify "$tmp" "$asset"

	tar -xzf "$tmp/$asset" -C "$tmp" tp
	mkdir -p "$INSTALL_DIR"
	# Write to a temp name then rename: safe even if tp is currently running.
	cp "$tmp/tp" "$INSTALL_DIR/.tp.new"
	chmod 0755 "$INSTALL_DIR/.tp.new"
	mv -f "$INSTALL_DIR/.tp.new" "$INSTALL_DIR/tp"

	installed="$("$INSTALL_DIR/tp" version 2>/dev/null | head -1 || true)"
	printf '%s\n' "${G}✔${N} installed ${B}${installed:-tp}${N} → $INSTALL_DIR/tp"

	case ":$PATH:" in
	*":$INSTALL_DIR:"*) ;;
	*)
		rc="$(shell_rc)"
		printf '\n%s\n' "${Y}!${N} $INSTALL_DIR is not on your PATH. Add it:"
		printf '%s\n' "    echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $rc && . $rc"
		;;
	esac

	printf '\n%s\n' "${B}Next${N}"
	printf '%s\n' "  tp providers     ${D}# add Anthropic or OpenAI: pick with ↑/↓, paste your API key${N}"
	printf '%s\n' "  cd your-project && tp"
	printf '%s\n' "${D}  docs: https://tp.0xabhi.space${N}"
}

detect_os() {
	case "$(uname -s)" in
	Darwin) echo darwin ;;
	Linux) echo linux ;;
	MINGW* | MSYS* | CYGWIN*)
		fail "Windows isn't supported natively yet. Install WSL (wsl --install) and run this command inside Ubuntu."
		;;
	*) fail "unsupported OS: $(uname -s) (supported: macOS, Linux)" ;;
	esac
}

detect_arch() {
	arch="$(uname -m)"
	# An x86_64 shell under Rosetta on Apple Silicon should still get arm64.
	if [ "$1" = darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
		arch=arm64
	fi
	case "$arch" in
	x86_64 | amd64) echo amd64 ;;
	arm64 | aarch64) echo arm64 ;;
	*) fail "unsupported CPU architecture: $arch (supported: x86_64, arm64)" ;;
	esac
}

download() {
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL --retry 3 -o "$2" "$1" || fail "download failed: $1"
	elif command -v wget >/dev/null 2>&1; then
		wget -q -O "$2" "$1" || fail "download failed: $1"
	else
		fail "curl or wget is required"
	fi
}

verify() {
	expected="$(grep "  $2\$" "$1/checksums.txt" | cut -d' ' -f1)"
	[ -n "$expected" ] || fail "no checksum for $2 in checksums.txt"
	if command -v shasum >/dev/null 2>&1; then
		actual="$(shasum -a 256 "$1/$2" | cut -d' ' -f1)"
	elif command -v sha256sum >/dev/null 2>&1; then
		actual="$(sha256sum "$1/$2" | cut -d' ' -f1)"
	else
		fail "shasum or sha256sum is required to verify the download"
	fi
	[ "$expected" = "$actual" ] || fail "checksum mismatch for $2 (expected $expected, got $actual)"
}

uninstall() {
	if [ -e "$INSTALL_DIR/tp" ]; then
		rm -f "$INSTALL_DIR/tp"
		echo "removed $INSTALL_DIR/tp"
	else
		echo "tp is not installed in $INSTALL_DIR"
	fi
	echo "your settings, sessions and saved keys were kept. To delete them too:"
	case "$(uname -s)" in
	Darwin) echo "  rm -rf \"\$HOME/Library/Application Support/theepori\" && security delete-generic-password -s theepori -a anthropic" ;;
	*) echo "  rm -rf \"\${XDG_CONFIG_HOME:-\$HOME/.config}/theepori\"" ;;
	esac
}

shell_rc() {
	case "${SHELL:-}" in
	*zsh) echo "$HOME/.zshrc" ;;
	*bash) [ "$(uname -s)" = Darwin ] && echo "$HOME/.bash_profile" || echo "$HOME/.bashrc" ;;
	*) echo "$HOME/.profile" ;;
	esac
}

fail() {
	printf '%s\n' "${R:-}✗ $*${N:-}" >&2
	exit 1
}

main "$@"
