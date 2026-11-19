#!/usr/bin/env bash
#
# Run one of the watcher's two services on this Mac, under Apple `container`
# (design decision 11). The launchd agents in launchd/ run this; so can you.
#
#   run-container.sh lane
#   run-container.sh api
#   run-container.sh --check lane|api    every precondition, then `ready`;
#                                        starts nothing
#
# **The NAS image, unchanged.** compose.yaml's two services, with what compose
# would provide done here instead:
#
#   compose                      here
#   restart: unless-stopped      launchd KeepAlive (this script stays in the
#                                foreground)
#   volume `data`                var/data, a host folder: Apple volumes are
#                                block devices, and two containers can't
#                                attach one at once
#   secrets                      var/secrets/<service>/, mounted at
#                                /run/secrets in that service's container only
#
# **`exec`, so a stop reaches the service.** launchd stops a job with
# SIGTERM. `container run` in the foreground passes it to the container's
# process, and --rm removes the container once that exits.
#
# launchd's environment is nearly empty, so PATH is set here and every path
# is absolute from this file's location.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"

IMAGE="${GALATA_WATCHER_IMAGE:-galata-watcher:latest}"
API_PORT="${GALATA_WATCHER_API_PORT:-8790}"
DATA="$ROOT/var/data"

refuse() { echo "run-container: REFUSED — $1" >&2; exit 2; }

CHECK=0
if [[ "${1:-}" == --check ]]; then CHECK=1; shift; fi
SERVICE="${1:-}"

case "$SERVICE" in
    lane)
        SECRET=telegram_token
        # The image's own command: cereyan serves the lane on the
        # container's loopback only.
        ARGS=()
        ;;
    api)
        SECRET=api_token
        # Bound to all of the container's interfaces so the published port
        # reaches it; published on the Mac's loopback only (below).
        ARGS=(galata-watcher-api --data /data --bind "0.0.0.0:$API_PORT")
        ;;
    *)
        refuse "usage: run-container.sh [--check] lane|api"
        ;;
esac
SECRETS="$ROOT/var/secrets/$SERVICE"
NAME="gw-$SERVICE"

command -v container >/dev/null || refuse "no \`container\` command; install Apple container (github.com/apple/container)"

# Start the container services if this login has not yet. A launchd agent
# can't answer a prompt, so the kernel is never installed from here.
if ! container system status >/dev/null 2>&1; then
    if (( CHECK )); then
        refuse "container services are stopped (container system start)"
    fi
    container system start --disable-kernel-install >/dev/null \
        || refuse "container system start failed; run it once by hand"
fi

container image inspect "$IMAGE" >/dev/null 2>&1 \
    || refuse "no image $IMAGE; build it with launchd/install.sh or \`container build -t $IMAGE .\`"
[[ -d "$DATA" ]] || refuse "no data folder at $DATA (mkdir -p var/data)"
[[ -f "$SECRETS/$SECRET" ]] || refuse "no secret file at $SECRETS/$SECRET"
[[ -s "$SECRETS/$SECRET" ]] || refuse "the secret file $SECRETS/$SECRET is empty"
mode="$(stat -f '%Lp' "$SECRETS/$SECRET")"
[[ "$mode" == 600 ]] || refuse "$SECRETS/$SECRET is mode $mode; it must be 600 (chmod 600)"
# Only this service's secret goes into its container.
extra="$(find "$SECRETS" -mindepth 1 ! -name "$SECRET" | head -1)"
[[ -z "$extra" ]] || refuse "$SECRETS holds more than $SECRET ($extra); only that file is mounted"

if (( CHECK )); then
    echo "ready: $NAME from $IMAGE, data $DATA, secret $SECRETS/$SECRET"
    exit 0
fi

# A container left over from a crash or a lost client still holds the name.
container rm -f "$NAME" >/dev/null 2>&1 || true

run=(container run --rm --name "$NAME"
     -v "$DATA:/data"
     -v "$SECRETS:/run/secrets")
if [[ "$SERVICE" == api ]]; then
    run+=(-p "127.0.0.1:$API_PORT:$API_PORT")
fi
exec "${run[@]}" "$IMAGE" "${ARGS[@]+"${ARGS[@]}"}"
