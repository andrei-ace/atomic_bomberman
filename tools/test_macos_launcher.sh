#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
mkdir -p build/swift-cache
xcrun swiftc -swift-version 5 -parse-as-library -D LAUNCHER_TESTS \
    -module-cache-path "$project_dir/build/swift-cache" \
    tools/macos-launcher.swift tools/macos-launcher-tests.swift -o build/macos-launcher-tests
build/macos-launcher-tests
