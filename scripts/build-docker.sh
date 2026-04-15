#!/usr/bin/env bash
#
# build-docker.sh [target]
# Runs scripts/build.sh inside a Debian builder container. This is the
# macOS-friendly entry point — Buildroot expects Linux, so we bring Linux.
#
# Requires Docker Desktop (or any Docker runtime) on the host.
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE_NAME="bunzo-builder:latest"

if ! command -v docker >/dev/null 2>&1; then
    echo "build-docker: docker is not installed or not on PATH" >&2
    echo "build-docker: install Docker Desktop (macOS) or your distro's docker package" >&2
    exit 1
fi

echo "build-docker: building builder image (cached after first run)"
docker build -t "${IMAGE_NAME}" -f "${REPO_ROOT}/Dockerfile.builder" "${REPO_ROOT}"

echo "build-docker: running build.sh ${TARGET} inside container"
docker run --rm -it \
    -v "${REPO_ROOT}:/src" \
    -w /src \
    --user "$(id -u):$(id -g)" \
    -e HOME=/tmp \
    "${IMAGE_NAME}" \
    /src/scripts/build.sh "${TARGET}"
