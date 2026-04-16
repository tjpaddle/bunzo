#!/usr/bin/env bash
#
# remote-build.sh [target]
#
# Drives a bunzo build on a remote Linux host from your laptop. Intended
# workflow: edit on macOS, push to GitHub, then run this script to pull
# and build on a beefier Linux box. Serial console of run-qemu.sh is
# handled separately by scripts/remote-qemu.sh.
#
# The build runs detached on the remote (setsid + nohup), writing to
# build.log, with its pid in build.pid and final exit code in build.exit.
# The local ssh session just tails build.log. If the SSH connection drops
# (consumer NAT, Wi-Fi blip, ISP CGNAT reset), this script auto-reconnects
# and re-attaches to the still-running build.
#
# Configure once by copying scripts/remote.env.example to
# scripts/remote.env.local and filling in your host details. That file is
# gitignored, so host/user/path never land in the public repo.
#
# Optional env overrides (take precedence over remote.env.local):
#   BUNZO_REMOTE_PUSH=0    skip the local `git push` step
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${REPO_ROOT}/scripts/remote.env.local"

if [[ ! -f "${ENV_FILE}" ]]; then
    echo "remote-build: ${ENV_FILE} not found" >&2
    echo "remote-build: cp scripts/remote.env.example scripts/remote.env.local and edit it" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "${ENV_FILE}"

BUNZO_REMOTE_HOST="${BUNZO_REMOTE_HOST:-filextract-server}"
BUNZO_REMOTE_USER="${BUNZO_REMOTE_USER:-filextract}"
BUNZO_REMOTE_PATH="${BUNZO_REMOTE_PATH:-/home/filextract/bunzo}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-2299}"
BUNZO_REMOTE_BRANCH="${BUNZO_REMOTE_BRANCH:-main}"
BUNZO_REMOTE_PUSH="${BUNZO_REMOTE_PUSH:-1}"

if [[ "${BUNZO_REMOTE_PUSH}" == "1" ]]; then
    CURRENT_BRANCH="$(git -C "${REPO_ROOT}" rev-parse --abbrev-ref HEAD)"
    if [[ "${CURRENT_BRANCH}" != "${BUNZO_REMOTE_BRANCH}" ]]; then
        echo "remote-build: WARNING local branch is '${CURRENT_BRANCH}' but remote will build '${BUNZO_REMOTE_BRANCH}'" >&2
    fi
    echo "remote-build: pushing ${CURRENT_BRANCH} to origin"
    git -C "${REPO_ROOT}" push origin "${CURRENT_BRANCH}"
fi

REPO_URL="$(git -C "${REPO_ROOT}" remote get-url origin)"

echo "remote-build: ssh -p ${BUNZO_REMOTE_PORT} ${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}"
echo "remote-build: target=${TARGET} branch=${BUNZO_REMOTE_BRANCH} path=${BUNZO_REMOTE_PATH}"
echo "remote-build: repo=${REPO_URL}"

# Wrap the ssh invocation in a function so the heredoc can be re-evaluated
# on each retry. Bash does NOT rewind a heredoc in a while-loop-around-`ssh`
# construct, but it does re-read it on every function call.
run_remote() {
    ssh \
        -o ServerAliveInterval=15 \
        -o ServerAliveCountMax=3 \
        -o ConnectTimeout=15 \
        -p "${BUNZO_REMOTE_PORT}" \
        "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
        "BUNZO_REMOTE_PATH='${BUNZO_REMOTE_PATH}' \
         BUNZO_REMOTE_BRANCH='${BUNZO_REMOTE_BRANCH}' \
         TARGET='${TARGET}' \
         REPO_URL='${REPO_URL}' \
         bash -s" <<'REMOTE'
set -euo pipefail

REMOTE_PATH="${BUNZO_REMOTE_PATH}"
BRANCH="${BUNZO_REMOTE_BRANCH}"

if [[ ! -d "${REMOTE_PATH}" ]]; then
    echo "[remote] cloning bunzo into ${REMOTE_PATH}"
    git clone "${REPO_URL}" "${REMOTE_PATH}"
fi

cd "${REMOTE_PATH}"
echo "[remote] pwd: $(pwd)"
echo "[remote] HEAD before pull: $(git rev-parse --short HEAD)"

git fetch origin
git checkout "${BRANCH}"
git pull --ff-only origin "${BRANCH}"
echo "[remote] HEAD after pull:  $(git rev-parse --short HEAD)"

if [[ ! -d buildroot ]]; then
    ./scripts/bootstrap.sh
fi

LOG_FILE="${REMOTE_PATH}/build.log"
EXIT_FILE="${REMOTE_PATH}/build.exit"
PID_FILE="${REMOTE_PATH}/build.pid"

# If a pid file exists but the process is gone, it's stale — drop it.
if [[ -f "${PID_FILE}" ]]; then
    STALE_PID="$(cat "${PID_FILE}" 2>/dev/null || true)"
    if [[ -z "${STALE_PID}" ]] || ! kill -0 "${STALE_PID}" 2>/dev/null; then
        rm -f "${PID_FILE}"
    fi
fi

if [[ -f "${PID_FILE}" ]]; then
    BUILD_PID="$(cat "${PID_FILE}")"
    echo "[remote] build already running (pid ${BUILD_PID}) — re-attaching to log"
else
    echo "[remote] starting detached build; log at ${LOG_FILE}"
    : > "${LOG_FILE}"
    rm -f "${EXIT_FILE}"
    # setsid + nohup + stdin from /dev/null = survives SSH disconnect.
    setsid nohup bash -c "
        cd '${REMOTE_PATH}'
        if [[ -f \"\${HOME}/.cargo/env\" ]]; then . \"\${HOME}/.cargo/env\"; fi
        ./scripts/build.sh '${TARGET}'
        echo \$? > '${EXIT_FILE}'
    " > "${LOG_FILE}" 2>&1 < /dev/null &
    BUILD_PID=$!
    echo "${BUILD_PID}" > "${PID_FILE}"
    disown || true
    sleep 1
fi

echo "[remote] tailing build.log (Ctrl-C here is safe — build keeps running)"
echo "----"
tail -n +1 -F --pid="${BUILD_PID}" "${LOG_FILE}" || true
echo "----"

if [[ -f "${EXIT_FILE}" ]]; then
    EXIT_CODE="$(cat "${EXIT_FILE}")"
    rm -f "${PID_FILE}"
    echo "[remote] build finished with exit code ${EXIT_CODE}"
    exit "${EXIT_CODE}"
else
    echo "[remote] build ended but no exit code recorded (see build.log)"
    exit 1
fi
REMOTE
}

# Retry loop: if ssh itself dies (code 255 = transport failure), reconnect
# and re-run the remote script. The remote script is idempotent — on
# reconnect it finds the still-running build via build.pid and re-tails
# build.log rather than starting a second build.
ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    if [[ "${ATTEMPT}" -gt 1 ]]; then
        echo "remote-build: reconnecting (attempt ${ATTEMPT})"
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
            echo "remote-build: interrupted by user (Ctrl-C). Build still running on remote — reconnect with ./scripts/remote-attach.sh"
            exit 130
            ;;
        255)
            echo "remote-build: ssh transport failure (code 255); reconnecting in 3s..."
            sleep 3
            ;;
        *)
            echo "remote-build: exited with code ${SSH_CODE}"
            exit "${SSH_CODE}"
            ;;
    esac
done
