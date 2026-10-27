#!/bin/sh
# Builds lemur-dictate, the helper lemur's dictation runs (src/dictate.swift).
# It needs Xcode 26 or newer, and runs on macOS 26 or newer.
set -eu
if [ "$#" -ne 1 ]; then
    printf 'Usage: %s destination\n' "$0" >&2
    exit 2
fi
cd "$(dirname "$0")/.."
xcrun swiftc -O -parse-as-library -target arm64-apple-macos26.0 \
    src/dictate.swift -o "$1"
