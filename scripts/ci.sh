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

step "Node: install"
pnpm install --frozen-lockfile

step "Biome: lint and format"
pnpm lint

step "TypeScript: typecheck"
pnpm typecheck

step "TypeScript: build"
pnpm build

step "Contract tests"
pnpm --filter @proxy-switcher/api test

step "Web: production build"
pnpm --filter web build

step "API: contract is served and complete"
# The contract lives in code now, so the gate is that what the server publishes
# still satisfies the schema the clients compile against. Regenerating from the
# live document must not change any operation.
curl -fsS --max-time 5 http://127.0.0.1:8088/readyz >/dev/null ||
  fail "API service is not ready on 127.0.0.1:8088"

DOC="$(mktemp)"
curl -fsS --max-time 5 http://127.0.0.1:8088/docs/json >"$DOC"

python3 - "$DOC" <<'PY'
import json, sys

doc = json.load(open(sys.argv[1]))
paths = doc.get("paths", {})

expected = {
    "/api/v1/active-proxy": {"get", "put", "delete"},
    "/api/v1/active-proxy/rotate": {"post"},
    "/api/v1/audit-events": {"get"},
    "/api/v1/proxies": {"get", "post"},
    "/api/v1/proxies/{id}": {"get", "delete"},
    "/api/v1/scrape-runs": {"post"},
    "/api/v1/scrape-runs/current": {"get"},
    "/api/v1/sessions": {"post"},
    "/api/v1/sessions/current": {"get", "delete"},
    "/api/v1/sources": {"get", "post"},
    "/api/v1/sources/{id}": {"delete"},
    "/api/v1/tokens": {"get", "post"},
    "/api/v1/tokens/exchange": {"post"},
    "/api/v1/tokens/{id}": {"delete"},
    "/api/v1/summary": {"get"},
    "/api/v1/stats": {"get"},
}

missing = []
for path, methods in sorted(expected.items()):
    actual = set(paths.get(path, {}))
    absent = methods - actual
    if absent:
        missing.append(f"{path}: {', '.join(sorted(absent))}")

# A colon in a route is a path parameter to the router, never a literal segment.
colon = [p for p in paths if ":" in p]

if colon:
    missing.append(f"routes using a colon, which the router reads as a parameter: {colon}")

if missing:
    print("contract mismatch:", file=sys.stderr)
    for line in missing:
        print(f"  - {line}", file=sys.stderr)
    sys.exit(1)
PY
rm -f "$DOC"
pass "contract complete"

printf '\n%sAll gates passed.%s\n' "$GREEN" "$RESET"