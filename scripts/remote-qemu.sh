#!/usr/bin/env bash
#
# remote-qemu.sh [target]
#
# Boots the most-recently-built bunzo image on the remote Linux host inside
# a tmux session, and attaches your terminal to that session via SSH. The
# QEMU process lives inside tmux on the remote, so an SSH disconnect leaves
# QEMU alive — re-running this script re-attaches to the same session.
#
# Inside the session:
#   Ctrl-B D    detach tmux (QEMU keeps running on the remote)
#   Ctrl-A X    exit QEMU cleanly (kills the tmux session too)
#   Ctrl-A C    open the QEMU monitor
#
# To kill a stuck session manually:
#   ssh -p <port> <user>@<host> 'tmux kill-session -t bunzo-qemu'
#
# Same host config as remote-build.sh (scripts/remote.env.local).
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${REPO_ROOT}/scripts/remote.env.local"

if [[ ! -f "${ENV_FILE}" ]]; then
    echo "remote-qemu: ${ENV_FILE} not found" >&2
    echo "remote-qemu: cp scripts/remote.env.example scripts/remote.env.local and edit it" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "${ENV_FILE}"

: "${BUNZO_REMOTE_HOST:?set BUNZO_REMOTE_HOST in scripts/remote.env.local}"
: "${BUNZO_REMOTE_USER:?set BUNZO_REMOTE_USER in scripts/remote.env.local}"
: "${BUNZO_REMOTE_PATH:?set BUNZO_REMOTE_PATH in scripts/remote.env.local}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-22}"

SESSION="bunzo-qemu"

echo "remote-qemu: target=${TARGET} on ${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}:${BUNZO_REMOTE_PORT}"
echo "remote-qemu: tmux session '${SESSION}' (Ctrl-B D detaches; Ctrl-A X exits QEMU)"

# `tmux new-session -A -s NAME 'cmd'` creates the session and runs cmd if it
# doesn't exist, or attaches to the existing session if it does. Either way
# the local terminal ends up attached.
run_remote() {
    ssh \
        -t \
        -o ServerAliveInterval=15 \
        -o ServerAliveCountMax=3 \
        -o ConnectTimeout=15 \
        -p "${BUNZO_REMOTE_PORT}" \
        "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
        "tmux new-session -A -s '${SESSION}' \"cd '${BUNZO_REMOTE_PATH}' && ./scripts/run-qemu.sh '${TARGET}'\""
}

ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    if [[ "${ATTEMPT}" -gt 1 ]]; then
        echo "remote-qemu: reconnecting (attempt ${ATTEMPT}) — QEMU still running in tmux"
    fi

    set +e
    run_remote
    SSH_CODE=$?
    set -e

    case "${SSH_CODE}" in
        0)
            exit 0
            ;;
        130)
            echo "remote-qemu: interrupted by user. QEMU may still be running in tmux — re-run this script to re-attach, or kill with: tmux kill-session -t ${SESSION}"
            exit 130
            ;;
        255)
            echo "remote-qemu: ssh transport failure; reconnecting in 2s..."
            sleep 2
            ;;
        *)
            exit "${SSH_CODE}"
            ;;
    esac
done
