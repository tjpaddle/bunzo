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

echo "post-build: bunzo rootfs verified ($(grep '^PRETTY_NAME=' "${TARGET_DIR}/etc/os-release"))"
