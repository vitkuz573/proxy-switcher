#!/usr/bin/env bash
set -euo pipefail

echo "Building proxy-switcher..."
cargo build --workspace

echo "Running tests..."
cargo test --workspace

echo "Running clippy..."
cargo clippy --workspace -- -D warnings

echo "Done."
