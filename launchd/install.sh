#!/usr/bin/env bash
#
# Install galata-watcher on this Mac as two launchd agents running Apple
# containers (design decision 11), or remove them.
#
#   launchd/install.sh              build the image, check, load both agents
#   launchd/install.sh --no-build   the same with the image already built
#   launchd/install.sh --uninstall  stop and remove both agents (keeps var/)
#
# Reinstalling is how an update is applied: the image is rebuilt and both
# agents restart on it. var/data (configuration, cursors, alert history) and
# var/secrets are kept.
#
# It never writes the Telegram token: create var/secrets/lane/telegram_token
# yourself (README, "Running on a Mac with Apple container"). It creates
# var/secrets/api/api_token if there is none, since any random value works.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"
IMAGE="${GALATA_WATCHER_IMAGE:-galata-watcher:latest}"
AGENTS="$HOME/Library/LaunchAgents"
DOMAIN="gui/$(id -u)"
SERVICES=(lane api)

say() { echo "install: $*"; }
refuse() { echo "install: REFUSED — $1" >&2; exit 2; }

unload() {
    local label="com.galata.watcher.$1"
    if launchctl print "$DOMAIN/$label" >/dev/null 2>&1; then
        launchctl bootout "$DOMAIN/$label" 2>/dev/null || true
        # bootout returns before the job is gone; wait so a reload can't race it.
        for _ in $(seq 1 30); do
            launchctl print "$DOMAIN/$label" >/dev/null 2>&1 || break
            sleep 1
        done
        say "unloaded $label"
    fi
}

BUILD=1
case "${1:-}" in
    "") ;;
    --no-build) BUILD=0 ;;
    --uninstall)
        for s in "${SERVICES[@]}"; do
            unload "$s"
            rm -f "$AGENTS/com.galata.watcher.$s.plist"
        done
        say "removed both agents; var/ is untouched"
        exit 0
        ;;
    *) refuse "usage: install.sh [--no-build | --uninstall]" ;;
esac

command -v container >/dev/null || refuse "no \`container\` command; install Apple container (github.com/apple/container)"
container system status >/dev/null 2>&1 || container system start --disable-kernel-install \
    || refuse "container system start failed; run it once by hand"

if (( BUILD )); then
    say "building $IMAGE (a first build takes several minutes)"
    container build -t "$IMAGE" "$ROOT"
fi

mkdir -p "$ROOT/var/data" "$ROOT/var/logs" "$ROOT/var/secrets/lane" "$ROOT/var/secrets/api"
chmod 700 "$ROOT/var/secrets" "$ROOT/var/secrets/lane" "$ROOT/var/secrets/api"
if [[ ! -e "$ROOT/var/secrets/api/api_token" ]]; then
    (umask 077 && openssl rand -hex 32 | tr -d '\n' > "$ROOT/var/secrets/api/api_token")
    say "created var/secrets/api/api_token (galata-desk will need it)"
fi

# Every precondition of both services before either is loaded.
for s in "${SERVICES[@]}"; do
    /bin/bash "$ROOT/scripts/run-container.sh" --check "$s"
done

mkdir -p "$AGENTS"
for s in "${SERVICES[@]}"; do
    label="com.galata.watcher.$s"
    unload "$s"
    sed "s|@ROOT@|$ROOT|g" "$ROOT/launchd/$label.plist" > "$AGENTS/$label.plist"
    plutil -lint -s "$AGENTS/$label.plist"
    launchctl bootstrap "$DOMAIN" "$AGENTS/$label.plist"
    say "loaded $label (log: var/logs/$s.log)"
done
say "done. The API answers on http://127.0.0.1:${GALATA_WATCHER_API_PORT:-8790} within a few seconds."
