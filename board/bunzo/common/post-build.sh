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

for unit in bunzo-shell.service bunzod.socket bunzod.service; do
    if [[ ! -f "${TARGET_DIR}/etc/systemd/system/${unit}" ]]; then
        echo "post-build: ${unit} missing from rootfs overlay" >&2
        exit 1
    fi
done

if [[ ! -f "${TARGET_DIR}/usr/lib/tmpfiles.d/bunzo.conf" ]]; then
    echo "post-build: tmpfiles.d/bunzo.conf missing from rootfs overlay" >&2
    exit 1
fi

if [[ ! -f "${TARGET_DIR}/etc/ssh/sshd_config" ]]; then
    echo "post-build: base sshd_config missing from rootfs overlay" >&2
    exit 1
fi

if ! grep -Eq '^[[:space:]]*Include[[:space:]]+/etc/ssh/sshd_config\.d/\*\.conf([[:space:]]|$)' \
    "${TARGET_DIR}/etc/ssh/sshd_config"; then
    echo "post-build: sshd_config does not load /etc/ssh/sshd_config.d/*.conf" >&2
    exit 1
fi

if [[ ! -f "${TARGET_DIR}/etc/ssh/sshd_config.d/bunzo-dev.conf" ]]; then
    echo "post-build: ssh dev drop-in missing from rootfs overlay" >&2
    exit 1
fi

for bin in bunzo-shell bunzod; do
    if [[ ! -x "${TARGET_DIR}/usr/bin/${bin}" ]]; then
        echo "post-build: /usr/bin/${bin} missing from rootfs" >&2
        echo "post-build: run cargo build before buildroot (see scripts/build.sh)" >&2
        exit 1
    fi
done

# Skills directory. M4 ships with read-local-file built in; anything placed
# under usr/lib/bunzo/skills/<name>/{manifest.toml,skill.wasm} is loaded by
# bunzod at startup.
SKILLS_DIR="${TARGET_DIR}/usr/lib/bunzo/skills"
if [[ -d "${SKILLS_DIR}" ]]; then
    while IFS= read -r -d '' skill; do
        name="$(basename "${skill}")"
        for f in manifest.toml skill.wasm; do
            if [[ ! -f "${skill}/${f}" ]]; then
                echo "post-build: skill ${name} missing ${f}" >&2
                exit 1
            fi
        done
        echo "post-build: skill ${name} ready"
    done < <(find "${SKILLS_DIR}" -mindepth 1 -maxdepth 1 -type d -print0)
else
    echo "post-build: no skills directory in rootfs (skipping)"
fi

echo "post-build: bunzo-shell.service enabled via [Install]; recovery via kernel cmdline 'bunzo.recovery'"
echo "post-build: bunzod.socket enabled via [Install]; bunzod.service is socket-activated (no [Install])"

echo "post-build: bunzo rootfs verified ($(grep '^PRETTY_NAME=' "${TARGET_DIR}/etc/os-release"))"
