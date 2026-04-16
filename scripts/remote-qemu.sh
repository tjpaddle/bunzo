#!/usr/bin/env bash
#
# remote-qemu.sh [target]
#
# Boots the most-recently-built bunzo image on the remote Linux host and
# streams its QEMU serial console back over SSH. Same host config as
# remote-build.sh (scripts/remote.env.local).
#
# Exit QEMU with Ctrl-A then X. If you get stuck, Ctrl-A C opens the QEMU
# monitor. SSH's own escape (~.) can collide; if it trips, reconnect with
# `ssh -e none ...` or edit this script.
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

echo "remote-qemu: target=${TARGET} on ${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}:${BUNZO_REMOTE_PORT}"
echo "remote-qemu: (Ctrl-A then X to exit QEMU)"

exec ssh -t -p "${BUNZO_REMOTE_PORT}" "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
    "cd '${BUNZO_REMOTE_PATH}' && ./scripts/run-qemu.sh '${TARGET}'"
