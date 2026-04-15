#!/usr/bin/env bash
#
# run-qemu.sh [target]
# Boots a built bunzo image in QEMU. Currently supports the qemu_aarch64
# target. Requires qemu-system-aarch64 on the host.
#
# macOS:  brew install qemu
# Linux:  apt install qemu-system-arm  (or your distro's equivalent)
#
# Exit QEMU with Ctrl-A then X.
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGES_DIR="${REPO_ROOT}/output/${TARGET}/images"

if [[ ! -d "${IMAGES_DIR}" ]]; then
    echo "run-qemu: ${IMAGES_DIR} not found" >&2
    echo "run-qemu: build the ${TARGET} target first (./scripts/build-docker.sh ${TARGET})" >&2
    exit 1
fi

case "${TARGET}" in
    qemu_aarch64)
        KERNEL="${IMAGES_DIR}/Image"
        ROOTFS="${IMAGES_DIR}/rootfs.ext4"
        for f in "${KERNEL}" "${ROOTFS}"; do
            [[ -f "${f}" ]] || { echo "run-qemu: missing ${f}" >&2; exit 1; }
        done

        if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
            echo "run-qemu: qemu-system-aarch64 not found on PATH" >&2
            echo "run-qemu: macOS -> brew install qemu   |   linux -> apt install qemu-system-arm" >&2
            exit 1
        fi

        echo "run-qemu: booting bunzo in QEMU (Ctrl-A then X to exit)"
        exec qemu-system-aarch64 \
            -M virt \
            -cpu cortex-a53 \
            -smp 2 \
            -m 1024 \
            -nographic \
            -kernel "${KERNEL}" \
            -append "root=/dev/vda rw console=ttyAMA0" \
            -drive file="${ROOTFS}",if=none,format=raw,id=hd0 \
            -device virtio-blk-device,drive=hd0 \
            -netdev user,id=net0 \
            -device virtio-net-device,netdev=net0
        ;;
    *)
        echo "run-qemu: target '${TARGET}' is not a QEMU target" >&2
        exit 1
        ;;
esac
