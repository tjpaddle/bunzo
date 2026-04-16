#!/usr/bin/env bash
#
# remote-attach.sh
#
# Reconnects to an in-flight remote bunzo build started by
# scripts/remote-build.sh. If a build is running (pidfile present and live),
# streams its log via tail -F until it finishes. If no build is running,
# prints the tail of the last log instead.
#
# Safe to run from any machine/network — the build lives in a setsid'd
# process on the remote, independent of any SSH tunnel.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${REPO_ROOT}/scripts/remote.env.local"

if [[ ! -f "${ENV_FILE}" ]]; then
    echo "remote-attach: ${ENV_FILE} not found" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "${ENV_FILE}"

: "${BUNZO_REMOTE_HOST:?set BUNZO_REMOTE_HOST in scripts/remote.env.local}"
: "${BUNZO_REMOTE_USER:?set BUNZO_REMOTE_USER in scripts/remote.env.local}"
: "${BUNZO_REMOTE_PATH:?set BUNZO_REMOTE_PATH in scripts/remote.env.local}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-22}"

exec ssh \
    -o ServerAliveInterval=30 \
    -o ServerAliveCountMax=10 \
    -p "${BUNZO_REMOTE_PORT}" \
    "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
    "BUNZO_REMOTE_PATH='${BUNZO_REMOTE_PATH}' bash -s" <<'REMOTE'
set -euo pipefail
LOG_FILE="${BUNZO_REMOTE_PATH}/build.log"
EXIT_FILE="${BUNZO_REMOTE_PATH}/build.exit"
PID_FILE="${BUNZO_REMOTE_PATH}/build.pid"

if [[ ! -f "${LOG_FILE}" ]]; then
    echo "remote-attach: no ${LOG_FILE} — run ./scripts/remote-build.sh first"
    exit 1
fi

if [[ -f "${PID_FILE}" ]]; then
    BUILD_PID="$(cat "${PID_FILE}" 2>/dev/null || true)"
    if [[ -n "${BUILD_PID}" ]] && kill -0 "${BUILD_PID}" 2>/dev/null; then
        echo "remote-attach: build running (pid ${BUILD_PID}); tailing ${LOG_FILE}"
        echo "----"
        tail -n +1 -F --pid="${BUILD_PID}" "${LOG_FILE}" || true
        echo "----"
        if [[ -f "${EXIT_FILE}" ]]; then
            echo "remote-attach: build finished with exit code $(cat "${EXIT_FILE}")"
        fi
        exit 0
    fi
fi

echo "remote-attach: no build currently running; showing last 200 lines of ${LOG_FILE}"
echo "----"
tail -n 200 "${LOG_FILE}"
if [[ -f "${EXIT_FILE}" ]]; then
    echo "----"
    echo "remote-attach: last build exit code: $(cat "${EXIT_FILE}")"
fi
REMOTE
