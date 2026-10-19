#!/usr/bin/env bash
#
# Build, stage and atomically release the API service.
#
# The previous deployment copied `dist/*.js` onto a live tree and left
# `node_modules` untouched, so a dependency bump silently ran the new code
# against old libraries. This script produces one self-contained release
# directory from the lockfile and swaps it in as a single step, which makes
# that class of drift impossible and gives instant rollback.
#
#   scripts/deploy-api.sh            # build, release, restart, verify
#   scripts/deploy-api.sh --no-restart
#   scripts/deploy-api.sh --rollback <release-id>
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_ROOT="$ROOT/.release"
DEPLOY_ROOT=/opt/proxy-switcher
RELEASES="$DEPLOY_ROOT/releases"
CURRENT="$DEPLOY_ROOT/current"
SERVICE=proxy-switcher-api
HEALTH_URL=http://127.0.0.1:8088/readyz
RELEASE_USER=proxy-switcher

RESTART=1
ROLLBACK_TO=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-restart) RESTART=0; shift ;;
    --rollback) ROLLBACK_TO="${2:?--rollback needs a release id}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ $EUID -ne 0 ]]; then
  echo "must run as root: sudo $0" >&2
  exit 1
fi

# The toolchain runs as the invoking user, not root: pnpm and npx keep per-user
# caches and stores, and running them as root pollutes both and breaks the
# symlinked layout `pnpm deploy` produces. PATH and HOME are passed explicitly
# because a bare `sudo -u` gives the child a minimal environment.
OWNER="${SUDO_USER:-root}"
OWNER_HOME="$(getent passwd "$OWNER" | cut -d: -f6)"
as_owner() {
  if [[ "$OWNER" == "root" ]]; then
    "$@"
  else
    sudo -u "$OWNER" env "PATH=$PATH" "HOME=$OWNER_HOME" "$@"
  fi
}

# ── Rollback ────────────────────────────────────────────────────────────────
if [[ -n "$ROLLBACK_TO" ]]; then
  target="$RELEASES/$ROLLBACK_TO"
  [[ -d "$target" ]] || { echo "no such release: $target" >&2; exit 1; }
  ln -sfn "$target" "$CURRENT"
  systemctl restart "$SERVICE"
  sleep 3
  systemctl is-active --quiet "$SERVICE" || { echo "rollback failed"; exit 1; }
  echo "rolled back to $ROLLBACK_TO"
  exit 0
fi

# ── Build ───────────────────────────────────────────────────────────────────
echo "==> type-checking and building (as $OWNER)"
as_owner bash -c "cd '$ROOT/api' && pnpm exec tsc --noEmit && pnpm --filter @proxy-switcher/api build"

# ── Stage ───────────────────────────────────────────────────────────────────
RELEASE_ID="$(date -u +%Y%m%dT%H%M%SZ)"
# Staged in the owner's own tree: `pnpm deploy` copies into its pnpm store and
# must not run as root.
STAGE="$STAGE_ROOT/$RELEASE_ID"
FINAL="$RELEASES/$RELEASE_ID"

mkdir -p "$RELEASES"
# The staging root must be owned by the build user: the script runs as root, so
# a plain mkdir would leave the deploy step unable to write into it.
as_owner mkdir -p "$STAGE_ROOT"
rm -rf "$STAGE"

echo "==> assembling release $RELEASE_ID"
# `pnpm deploy --prod` resolves from the workspace lockfile and produces a
# self-contained tree: production dependencies only, no workspace links.
as_owner bash -c "cd '$ROOT' && pnpm --filter @proxy-switcher/api deploy --prod '$STAGE'"

# ── Swap ────────────────────────────────────────────────────────────────────
if [[ -L "$CURRENT" ]]; then
  echo "==> previous release: $(basename "$(readlink -f "$CURRENT")")"
fi

mv "$STAGE" "$FINAL"
chown -R "$RELEASE_USER:$RELEASE_USER" "$FINAL"
chmod -R go-w "$FINAL"

ln -sfn "$FINAL" "$CURRENT"

if [[ $RESTART -eq 0 ]]; then
  echo "==> staged at $FINAL (not restarting)"
  exit 0
fi

# ── Verify ──────────────────────────────────────────────────────────────────
echo "==> restarting $SERVICE"
systemctl restart "$SERVICE"

for _ in $(seq 1 20); do
  if curl -fsS --max-time 2 "$HEALTH_URL" >/dev/null 2>&1; then
    echo "==> live: $CURRENT -> $RELEASE_ID"
    exit 0
  fi
  sleep 1
done

echo "!!! $SERVICE did not become ready" >&2
systemctl status "$SERVICE" --no-pager >&2 || true
journalctl -u "$SERVICE" --no-pager -n 30 >&2 || true
exit 1