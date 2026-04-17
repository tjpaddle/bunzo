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

# --- bunzo userland (Rust) ---
# Build bunzo's own binaries first and stage them into the rootfs overlay so
# Buildroot picks them up during image assembly. Target triple is derived
# from the Buildroot target name; qemu_aarch64 and rpi4 both map to
# aarch64-unknown-linux-musl for a fully static binary.
case "${TARGET}" in
    qemu_aarch64|rpi4) RUST_TARGET="aarch64-unknown-linux-musl" ;;
    pc_x86_64)         RUST_TARGET="x86_64-unknown-linux-musl" ;;
    *)                 RUST_TARGET="" ;;
esac

if [[ -n "${RUST_TARGET}" && -f "${REPO_ROOT}/rust/Cargo.toml" ]]; then
    echo "build: cargo build bunzo userland for ${RUST_TARGET}"
    (
        cd "${REPO_ROOT}/rust"
        cargo build --release --target "${RUST_TARGET}" -p bunzo-shell -p bunzod
    )
    CARGO_BIN_BASE="${CARGO_TARGET_DIR:-${REPO_ROOT}/rust/target}"
    OVERLAY_BIN_DIR="${BOARD_DIR}/bunzo/common/rootfs-overlay/usr/bin"
    mkdir -p "${OVERLAY_BIN_DIR}"
    for bin in bunzo-shell bunzod; do
        BIN_PATH="${CARGO_BIN_BASE}/${RUST_TARGET}/release/${bin}"
        if [[ ! -x "${BIN_PATH}" ]]; then
            echo "build: cargo build succeeded but ${BIN_PATH} is missing" >&2
            exit 1
        fi
        install -m 0755 "${BIN_PATH}" "${OVERLAY_BIN_DIR}/${bin}"
    done
    echo "build: staged bunzo-shell + bunzod into overlay"
fi

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
