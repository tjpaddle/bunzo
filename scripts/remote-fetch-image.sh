#!/usr/bin/env bash
#
# remote-fetch-image.sh [target] [destination-dir] [--dry-run]
#
# Fetches a hardware disk image built by scripts/remote-build.sh from the
# configured remote Linux builder. The default target is the first hardware
# replay target, cm5_nano_a, whose Buildroot post-image step produces
# output/cm5_nano_a/images/sdcard.img.
#
# The script reads the same scripts/remote.env.local file as remote-build.sh.
# It does not build, flash, or mutate the remote tree.
#
set -euo pipefail

TARGET=""
DEST_DIR=""
DRY_RUN=0

for arg in "$@"; do
    case "${arg}" in
        --dry-run)
            DRY_RUN=1
            ;;
        -*)
            echo "remote-fetch-image: unknown option '${arg}'" >&2
            exit 2
            ;;
        *)
            if [[ -z "${TARGET}" ]]; then
                TARGET="${arg}"
            elif [[ -z "${DEST_DIR}" ]]; then
                DEST_DIR="${arg}"
            else
                echo "remote-fetch-image: unexpected positional arg '${arg}'" >&2
                exit 2
            fi
            ;;
    esac
done

TARGET="${TARGET:-cm5_nano_a}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST_DIR="${DEST_DIR:-${REPO_ROOT}/dist/${TARGET}}"
ENV_FILE="${REPO_ROOT}/scripts/remote.env.local"

if [[ ! -f "${ENV_FILE}" ]]; then
    echo "remote-fetch-image: ${ENV_FILE} not found" >&2
    echo "remote-fetch-image: cp scripts/remote.env.example scripts/remote.env.local and edit it" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "${ENV_FILE}"

BUNZO_REMOTE_HOST="${BUNZO_REMOTE_HOST:-filextract-server}"
BUNZO_REMOTE_USER="${BUNZO_REMOTE_USER:-filextract}"
BUNZO_REMOTE_PATH="${BUNZO_REMOTE_PATH:-/home/filextract/bunzo}"
BUNZO_REMOTE_PORT="${BUNZO_REMOTE_PORT:-2299}"

REMOTE_IMAGE="${BUNZO_REMOTE_PATH}/output/${TARGET}/images/sdcard.img"
LOCAL_IMAGE="${DEST_DIR}/sdcard.img"
LOCAL_SHA="${LOCAL_IMAGE}.sha256"

echo "remote-fetch-image: target=${TARGET}"
echo "remote-fetch-image: remote=${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}:${REMOTE_IMAGE}"
echo "remote-fetch-image: destination=${LOCAL_IMAGE}"

REMOTE_SHA_LINE="$(
    ssh \
        -o ServerAliveInterval=15 \
        -o ServerAliveCountMax=3 \
        -o ConnectTimeout=15 \
        -p "${BUNZO_REMOTE_PORT}" \
        "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}" \
        "BUNZO_REMOTE_IMAGE='${REMOTE_IMAGE}' bash -s" <<'REMOTE'
set -euo pipefail
test -f "${BUNZO_REMOTE_IMAGE}"
sha256sum "${BUNZO_REMOTE_IMAGE}"
REMOTE
)"

REMOTE_SHA="${REMOTE_SHA_LINE%% *}"
echo "remote-fetch-image: remote sha256=${REMOTE_SHA}"

if [[ "${DRY_RUN}" == "1" ]]; then
    echo "remote-fetch-image: dry run complete; image exists on remote"
    exit 0
fi

mkdir -p "${DEST_DIR}"
scp \
    -P "${BUNZO_REMOTE_PORT}" \
    "${BUNZO_REMOTE_USER}@${BUNZO_REMOTE_HOST}:${REMOTE_IMAGE}" \
    "${LOCAL_IMAGE}"

printf '%s  %s\n' "${REMOTE_SHA}" "$(basename "${LOCAL_IMAGE}")" > "${LOCAL_SHA}"
echo "remote-fetch-image: wrote ${LOCAL_IMAGE}"
echo "remote-fetch-image: wrote ${LOCAL_SHA}"
