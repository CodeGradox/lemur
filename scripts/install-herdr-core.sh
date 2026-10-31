#!/bin/sh
# Installs herdr-core into ~/.local/bin for the tests, which drive real
# herdr-core sessions: this version for Apple Silicon, checked against its
# release manifest's checksum
# (https://releases.herdr.dev/core/0.1.15/manifest.json).
set -eu

version=0.1.15
sha256=47eba8367e0626a1efced8795fcb03d5434b2541c5a84d78c50cc02c8fb49fc8
url="https://releases.herdr.dev/core/$version/herdr-core-macos-aarch64.tar.gz"

test "$(uname -s)-$(uname -m)" = Darwin-arm64
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/hc.tar.gz"
echo "$sha256  $tmp/hc.tar.gz" | shasum -a 256 -c -
tar -xzf "$tmp/hc.tar.gz" -C "$tmp"
mkdir -p "$HOME/.local/bin"
install -m 755 "$tmp/herdr-core" "$HOME/.local/bin/herdr-core"
"$HOME/.local/bin/herdr-core" --version
