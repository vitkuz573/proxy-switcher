#!/usr/bin/env bash
#
# Build, stage and atomically release the web console.
#
# Next.js `output: "standalone"` produces a self-contained server bundle with a
# pruned dependency tree, so the release directory carries no monorepo links and
# can be swapped as a unit.
#
#   scripts/deploy-web.sh
#   scripts/deploy-web.sh --no-restart
#   scripts/deploy-web.sh --rollback <release-id>
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_ROOT="$ROOT/.release-web"
DEPLOY_ROOT=/opt/proxy-switcher
RELEASES="$DEPLOY_ROOT/releases-web"
CURRENT="$DEPLOY_ROOT/current-web"
SERVICE=proxy-switcher-web
HEALTH_URL="http://127.0.0.1:${WEB_PORT:-3100}/login"
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
as_owner bash -c "cd '$ROOT/web' && pnpm exec tsc --noEmit && pnpm build"

RELEASE_ID="$(date -u +%Y%m%dT%H%M%SZ)"
STAGE="$STAGE_ROOT/$RELEASE_ID"
FINAL="$RELEASES/$RELEASE_ID"

mkdir -p "$RELEASES"
as_owner mkdir -p "$STAGE_ROOT"
rm -rf "$STAGE"

# ── Stage ───────────────────────────────────────────────────────────────────
echo "==> assembling release $RELEASE_ID"

# Next's standalone tracer copies pnpm's symlinked layout verbatim, which leaves
# nested transitive packages unresolved once the monorepo is gone.
# `pnpm deploy --prod` resolves from the lockfile and produces a real,
# self-contained tree, so the build output is layered on top of that.
as_owner bash -c "cd '$ROOT' && pnpm --filter web deploy --prod '$STAGE'"

as_owner bash -c "
  set -e
  cd '$ROOT/web'

  # The whole build directory is needed: next start reads BUILD_ID, the server
  # output and the route manifests, not just the static assets.
  rm -rf '$STAGE/.next'
  mkdir -p '$STAGE/.next'
  (cd .next && find . -mindepth 1 -maxdepth 1 ! -name cache ! -name standalone -exec cp -RL . '{}' '$STAGE/.next/' \;)

  if [ -d public ]; then cp -RL public '$STAGE/public'; fi
  rm -rf '$STAGE/node_modules/.bin'
"

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

for _ in $(seq 1 30); do
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