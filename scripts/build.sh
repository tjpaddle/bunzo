#!/usr/bin/env bash
#
# build.sh [target]
# Builds a bunzo image for the given target.
# Default target: qemu_aarch64.
#
# Must run on Linux with Buildroot's host dependencies installed.
# On macOS, use ./scripts/build-docker.sh instead, which wraps this.
#
# build-docker.sh sets BUNZO_OUTPUT_BASE, BUNZO_DL_DIR, and BUNZO_HOST_OUTPUT
# to route heavy I/O onto Docker named volumes instead of the macOS virtiofs
# bind mount (which causes SIGBUS under Buildroot's write pattern). Native
# Linux builds leave those unset and write straight into the repo tree.
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILDROOT_DIR="${REPO_ROOT}/buildroot"
BOARD_DIR="${REPO_ROOT}/board"
OUTPUT_BASE="${BUNZO_OUTPUT_BASE:-${REPO_ROOT}/output}"
OUTPUT_DIR="${OUTPUT_BASE}/${TARGET}"
DEFCONFIG="bunzo_${TARGET}_defconfig"

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "build: buildroot not present at ${BUILDROOT_DIR}" >&2
    echo "build: run ./scripts/bootstrap.sh first" >&2
    exit 1
fi

if [[ ! -f "${BOARD_DIR}/configs/${DEFCONFIG}" ]]; then
    echo "build: unknown target '${TARGET}' (no ${DEFCONFIG} found)" >&2
    echo "build: available targets:" >&2
    ls "${BOARD_DIR}/configs/" 2>/dev/null | sed 's/^bunzo_//;s/_defconfig$//;s/^/  /' >&2
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"

MAKE_ARGS=(
    -C "${BUILDROOT_DIR}"
    BR2_EXTERNAL="${BOARD_DIR}"
    O="${OUTPUT_DIR}"
)
if [[ -n "${BUNZO_DL_DIR:-}" ]]; then
    mkdir -p "${BUNZO_DL_DIR}"
    MAKE_ARGS+=(BR2_DL_DIR="${BUNZO_DL_DIR}")
fi

echo "build: configuring buildroot for ${TARGET} (output=${OUTPUT_DIR})"
make "${MAKE_ARGS[@]}" "${DEFCONFIG}"

echo "build: starting full build (first run: expect 30-90 minutes)"
make "${MAKE_ARGS[@]}"

if [[ -n "${BUNZO_HOST_OUTPUT:-}" ]]; then
    HOST_IMAGES_DIR="${BUNZO_HOST_OUTPUT}/${TARGET}/images"
    echo "build: copying images/ to host at ${HOST_IMAGES_DIR}"
    mkdir -p "${BUNZO_HOST_OUTPUT}/${TARGET}"
    rm -rf "${HOST_IMAGES_DIR}"
    cp -r "${OUTPUT_DIR}/images" "${HOST_IMAGES_DIR}"
    FINAL_IMAGES_DIR="${HOST_IMAGES_DIR}"
else
    FINAL_IMAGES_DIR="${OUTPUT_DIR}/images"
fi

echo
echo "build: done. artifacts in ${FINAL_IMAGES_DIR}"
ls -lh "${FINAL_IMAGES_DIR}" 2>/dev/null || true
