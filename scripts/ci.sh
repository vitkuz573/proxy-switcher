#!/usr/bin/env bash
# Local CI. GitHub Actions are blocked in Russia (project constraint), so the
# same gates run here instead.
set -euo pipefail

cd "$(dirname "$0")/.."

RED=$'\033[31m'
GREEN=$'\033[32m'
DIM=$'\033[2m'
RESET=$'\033[0m'

step() { printf '\n%s▸ %s%s\n' "$DIM" "$1" "$RESET"; }
fail() { printf '%s✗ %s%s\n' "$RED" "$1" "$RESET"; exit 1; }
pass() { printf '%s✓ %s%s\n' "$GREEN" "$1" "$RESET"; }

step "Rust: format"
cargo fmt --all -- --check

step "Rust: clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "Rust: tests"
cargo test --workspace

step "TypeScript: install"
pnpm install --frozen-lockfile

step "TypeScript: typecheck"
pnpm -r typecheck

step "TypeScript: build"
pnpm -r build

step "OpenAPI spec matches the committed SDK"
# Regenerating must be a no-op; a diff here means someone changed a route
# without refreshing the client.
cp sdk/src/schema.d.ts sdk/src/schema.d.ts.bak
pnpm --filter @proxy-switcher/sdk generate >/dev/null 2>&1 || true
if ! diff -q sdk/src/schema.d.ts sdk/src/schema.d.ts.bak >/dev/null 2>&1; then
  mv sdk/src/schema.d.ts.bak sdk/src/schema.d.ts
  fail "sdk/src/schema.d.ts is stale — run 'pnpm sdk:generate' with the API up"
fi
rm -f sdk/src/schema.d.ts.bak

printf '\n%sAll gates passed.%s\n' "$GREEN" "$RESET"