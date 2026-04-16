#!/usr/bin/env bash
#
# remote-build.sh [target]
#
# Drives a bunzo build on a remote Linux host from your laptop. Intended
# workflow: edit on macOS, push to GitHub, then run this script to pull
# and build on a beefier Linux box. Serial console of run-qemu.sh is
# handled separately by scripts/remote-qemu.sh.
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

: "${BUNZO_REMOTE_HOST:?set BUNZO_REMOTE_HOST in scripts/remote.env.local}"
: "${BUNZO_REMOTE_USER:?set BUNZO_REMOTE_USER in scripts/remote.env.local}"
: "${BUNZO_REMOTE_PATH:?set BUNZO_REMOTE_PATH in scripts/remote.env.local}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-22}"
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

echo "remote-build: ssh -p ${BUNZO_REMOTE_PORT} ${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}"
echo "remote-build: target=${TARGET} branch=${BUNZO_REMOTE_BRANCH} path=${BUNZO_REMOTE_PATH}"

# Single ssh invocation → single password prompt (if no ssh key).
# Heredoc is unquoted on purpose so the client-side vars expand here;
# anything that must evaluate on the remote is written as \$VAR.
ssh -p "${BUNZO_REMOTE_PORT}" "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" bash -s <<REMOTE
set -euo pipefail

REMOTE_PATH="${BUNZO_REMOTE_PATH}"
BRANCH="${BUNZO_REMOTE_BRANCH}"
TARGET="${TARGET}"

if [[ ! -d "\${REMOTE_PATH}" ]]; then
    echo "[remote] cloning bunzo into \${REMOTE_PATH}"
    git clone https://github.com/tjpatel0397/bunzo.git "\${REMOTE_PATH}"
fi

cd "\${REMOTE_PATH}"
echo "[remote] pwd: \$(pwd)"
echo "[remote] HEAD before pull: \$(git rev-parse --short HEAD)"

git fetch origin
git checkout "\${BRANCH}"
git pull --ff-only origin "\${BRANCH}"
echo "[remote] HEAD after pull:  \$(git rev-parse --short HEAD)"

if [[ ! -d buildroot ]]; then
    ./scripts/bootstrap.sh
fi

./scripts/build.sh "\${TARGET}"
echo "[remote] build complete for \${TARGET}"
REMOTE

echo "remote-build: done"
echo "remote-build: to boot in QEMU over SSH: ./scripts/remote-qemu.sh ${TARGET}"
