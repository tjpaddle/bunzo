#!/usr/bin/env bash
#
# build.sh [target]
# Builds a bunzo image for the given target.
# Default target: qemu_aarch64.
#
# Must run on Linux with Buildroot's host dependencies installed.
# On macOS, use ./scripts/build-docker.sh instead, which wraps this.
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILDROOT_DIR="${REPO_ROOT}/buildroot"
BOARD_DIR="${REPO_ROOT}/board"
OUTPUT_DIR="${REPO_ROOT}/output/${TARGET}"
DEFCONFIG="bunzo_${TARGET}_defconfig"

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "build: buildroot not present at ${BUILDROOT_DIR}" >&2
    echo "build: run ./scripts/bootstrap.sh first" >&2
    exit 1
fi

if [[ ! -f "${BOARD_DIR}/bunzo/configs/${DEFCONFIG}" ]]; then
    echo "build: unknown target '${TARGET}' (no ${DEFCONFIG} found)" >&2
    echo "build: available targets:" >&2
    ls "${BOARD_DIR}/bunzo/configs/" 2>/dev/null | sed 's/^bunzo_//;s/_defconfig$//;s/^/  /' >&2
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"

echo "build: configuring buildroot for ${TARGET}"
make -C "${BUILDROOT_DIR}" \
    BR2_EXTERNAL="${BOARD_DIR}" \
    O="${OUTPUT_DIR}" \
    "${DEFCONFIG}"

echo "build: starting full build (first run: expect 30-90 minutes)"
make -C "${BUILDROOT_DIR}" \
    BR2_EXTERNAL="${BOARD_DIR}" \
    O="${OUTPUT_DIR}"

echo
echo "build: done. artifacts in ${OUTPUT_DIR}/images/"
ls -lh "${OUTPUT_DIR}/images/" 2>/dev/null || true
