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
# process on the remote, independent of any SSH tunnel. If the SSH
# connection drops mid-tail, this script auto-reconnects.
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

run_remote() {
    ssh \
        -o ServerAliveInterval=15 \
        -o ServerAliveCountMax=3 \
        -o ConnectTimeout=15 \
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
}

ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    if [[ "${ATTEMPT}" -gt 1 ]]; then
        echo "remote-attach: reconnecting (attempt ${ATTEMPT})"
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
            echo "remote-attach: interrupted by user"
            exit 130
            ;;
        255)
            echo "remote-attach: ssh transport failure (code 255); reconnecting in 3s..."
            sleep 3
            ;;
        *)
            exit "${SSH_CODE}"
            ;;
    esac
done
