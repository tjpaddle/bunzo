#!/usr/bin/env bash
#
# post-build.sh
# Runs after Buildroot assembles the rootfs, before image packaging.
# Argument $1 is the target rootfs path (TARGET_DIR).
#
set -euo pipefail

TARGET_DIR="$1"

for f in /etc/os-release /etc/motd /etc/hostname; do
    if [[ ! -f "${TARGET_DIR}${f}" ]]; then
        echo "post-build: missing ${f} in bunzo rootfs" >&2
        exit 1
    fi
done

if [[ -f "${TARGET_DIR}/etc/systemd/system/bunzo-shell.service" ]]; then
    mkdir -p "${TARGET_DIR}/etc/systemd/system/multi-user.target.wants"
    ln -sf ../bunzo-shell.service \
        "${TARGET_DIR}/etc/systemd/system/multi-user.target.wants/bunzo-shell.service"
    ln -sf /dev/null \
        "${TARGET_DIR}/etc/systemd/system/serial-getty@ttyAMA0.service"
    echo "post-build: enabled bunzo-shell.service, masked serial-getty@ttyAMA0"
fi

if [[ ! -x "${TARGET_DIR}/usr/bin/bunzo-shell" ]]; then
    echo "post-build: /usr/bin/bunzo-shell missing from rootfs" >&2
    echo "post-build: run cargo build before buildroot (see scripts/build.sh)" >&2
    exit 1
fi

echo "post-build: bunzo rootfs verified ($(grep '^PRETTY_NAME=' "${TARGET_DIR}/etc/os-release"))"
