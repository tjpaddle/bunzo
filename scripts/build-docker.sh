#!/usr/bin/env bash
#
# build-docker.sh [target]
# Runs scripts/build.sh inside a Debian builder container. macOS-friendly
# entry point — Buildroot expects Linux, so we bring Linux.
#
# Heavy Buildroot writes (output/ and dl/) go to Docker named volumes, not
# the bind-mounted repo, because macOS virtiofs mmap misbehaves under
# Buildroot's write pattern and takes the container down with SIGBUS / EOF.
# Final images are copied back onto the host bind mount at the end of the
# build so run-qemu.sh and host tooling can find them as usual.
#
# Requires Docker Desktop (or any Docker runtime) on the host.
#
set -euo pipefail

TARGET="${1:-qemu_aarch64}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE_NAME="bunzo-builder:latest"
OUTPUT_VOL="bunzo-output"
DL_VOL="bunzo-dl"
CARGO_VOL="bunzo-cargo"
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

if ! command -v docker >/dev/null 2>&1; then
    echo "build-docker: docker is not installed or not on PATH" >&2
    echo "build-docker: install Docker Desktop (macOS) or your distro's docker package" >&2
    exit 1
fi

echo "build-docker: building builder image (cached after first run)"
docker build -t "${IMAGE_NAME}" -f "${REPO_ROOT}/Dockerfile.builder" "${REPO_ROOT}"

echo "build-docker: ensuring named volumes exist (${OUTPUT_VOL}, ${DL_VOL}, ${CARGO_VOL})"
docker volume create "${OUTPUT_VOL}" >/dev/null
docker volume create "${DL_VOL}" >/dev/null
docker volume create "${CARGO_VOL}" >/dev/null

echo "build-docker: chowning volumes to ${HOST_UID}:${HOST_GID}"
docker run --rm \
    -v "${OUTPUT_VOL}:/bunzo-output" \
    -v "${DL_VOL}:/bunzo-dl" \
    -v "${CARGO_VOL}:/bunzo-cargo" \
    --user 0:0 \
    "${IMAGE_NAME}" \
    chown -R "${HOST_UID}:${HOST_GID}" /bunzo-output /bunzo-dl /bunzo-cargo

echo "build-docker: running build.sh ${TARGET} inside container"
docker run --rm -it \
    --shm-size=2g \
    -v "${REPO_ROOT}:/src" \
    -v "${OUTPUT_VOL}:/bunzo-output" \
    -v "${DL_VOL}:/bunzo-dl" \
    -v "${CARGO_VOL}:/bunzo-cargo" \
    -w /src \
    --user "${HOST_UID}:${HOST_GID}" \
    -e HOME=/tmp \
    -e BUNZO_OUTPUT_BASE=/bunzo-output \
    -e BUNZO_DL_DIR=/bunzo-dl \
    -e BUNZO_HOST_OUTPUT=/src/output \
    -e CARGO_HOME=/bunzo-cargo \
    -e CARGO_TARGET_DIR=/bunzo-cargo/target \
    "${IMAGE_NAME}" \
    /src/scripts/build.sh "${TARGET}"
