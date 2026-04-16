#!/usr/bin/env bash
#
# remote-qemu.sh [target]
#
# Boots the most-recently-built bunzo image on the remote Linux host and
# attaches your terminal to it over SSH.
#
# Default mode is a direct SSH session with no tmux involved. QEMU is tied to
# the SSH session, so closing the terminal or dropping the connection ends the
# remote QEMU process too. This matches the normal "run it and watch it" loop.
#
# Optional env override:
#   BUNZO_REMOTE_QEMU_PERSIST=1   run QEMU inside tmux so SSH drops do not kill it
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

BUNZO_REMOTE_HOST="${BUNZO_REMOTE_HOST:-filextract-server}"
BUNZO_REMOTE_USER="${BUNZO_REMOTE_USER:-filextract}"
BUNZO_REMOTE_PATH="${BUNZO_REMOTE_PATH:-/home/filextract/bunzo}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-2299}"
BUNZO_REMOTE_QEMU_PERSIST="${BUNZO_REMOTE_QEMU_PERSIST:-0}"

SESSION="bunzo-qemu"

echo "remote-qemu: target=${TARGET} on ${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}:${BUNZO_REMOTE_PORT}"
if [[ "${BUNZO_REMOTE_QEMU_PERSIST}" == "1" ]]; then
    echo "remote-qemu: persistent mode via tmux session '${SESSION}' (Ctrl-B D detaches; Ctrl-A X exits QEMU)"
else
    echo "remote-qemu: direct mode (no tmux). Ctrl-A X exits QEMU; closing SSH ends it."
fi

run_remote() {
    if [[ "${BUNZO_REMOTE_QEMU_PERSIST}" == "1" ]]; then
        ssh \
            -t \
            -o ServerAliveInterval=15 \
            -o ServerAliveCountMax=3 \
            -o ConnectTimeout=15 \
            -p "${BUNZO_REMOTE_PORT}" \
            "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
            "tmux new-session -A -s '${SESSION}' \"cd '${BUNZO_REMOTE_PATH}' && ./scripts/run-qemu.sh '${TARGET}'\""
    else
        ssh \
            -t \
            -o ServerAliveInterval=15 \
            -o ServerAliveCountMax=3 \
            -o ConnectTimeout=15 \
            -p "${BUNZO_REMOTE_PORT}" \
            "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
            "cd '${BUNZO_REMOTE_PATH}' && ./scripts/run-qemu.sh '${TARGET}'"
    fi
}

if [[ "${BUNZO_REMOTE_QEMU_PERSIST}" != "1" ]]; then
    exec ssh \
        -t \
        -o ServerAliveInterval=15 \
        -o ServerAliveCountMax=3 \
        -o ConnectTimeout=15 \
        -p "${BUNZO_REMOTE_PORT}" \
        "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
        "cd '${BUNZO_REMOTE_PATH}' && ./scripts/run-qemu.sh '${TARGET}'"
fi

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
