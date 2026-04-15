#!/usr/bin/env bash
#
# bootstrap.sh
# Clones Buildroot into ./buildroot/ at a pinned release branch.
# Re-runnable: does nothing if the tree is already present.
#
# Override the branch with: BUILDROOT_BRANCH=2026.02.x ./scripts/bootstrap.sh
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILDROOT_DIR="${REPO_ROOT}/buildroot"
BUILDROOT_BRANCH="${BUILDROOT_BRANCH:-2025.02.x}"
BUILDROOT_URL="${BUILDROOT_URL:-https://git.busybox.net/buildroot}"

if [[ -d "${BUILDROOT_DIR}/.git" ]]; then
    echo "bootstrap: buildroot already cloned at ${BUILDROOT_DIR}"
    echo "bootstrap: (to switch branch: rm -rf ${BUILDROOT_DIR} && BUILDROOT_BRANCH=... ./scripts/bootstrap.sh)"
    exit 0
fi

echo "bootstrap: cloning buildroot branch ${BUILDROOT_BRANCH} from ${BUILDROOT_URL}"
git clone --depth 1 --branch "${BUILDROOT_BRANCH}" "${BUILDROOT_URL}" "${BUILDROOT_DIR}"
echo "bootstrap: done — buildroot is at ${BUILDROOT_DIR}"
