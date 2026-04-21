#!/usr/bin/env bash
#
# measure-bunzod-rss.sh
# Samples bunzod's RSS inside a running QEMU image via the hostfwd SSH
# port set up by run-qemu.sh (localhost:2222 -> guest:22).
#
# Usage:
#   ./scripts/measure-bunzod-rss.sh
# Requires:
#   - The QEMU image built with the bunzo-dev sshd drop-in (default).
#   - ssh on the host. An empty password is accepted by the dev image.
#
set -euo pipefail

PORT="${BUNZO_SSH_PORT:-2222}"
HOST="${BUNZO_SSH_HOST:-localhost}"

SSH_ARGS=(
    -p "${PORT}"
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
)

echo "measure-bunzod-rss: ssh root@${HOST}:${PORT} (hit Enter at the password prompt)"
ssh "${SSH_ARGS[@]}" "root@${HOST}" '
set -eu
pid="$(pidof bunzod 2>/dev/null | awk '"'"'{print $1}'"'"')"
if [ -z "${pid:-}" ]; then
    pid="$(ps | awk '"'"'$3 == "bunzod" { print $1; exit }'"'"')"
fi
if [ -z "${pid:-}" ]; then
    echo "bunzod is not running"
    exit 2
fi
echo "pid: ${pid}"
grep -E "^(Name|Vm(RSS|HWM|Peak|Size|Data|Stk|Exe|Lib|PTE)):" "/proc/${pid}/status"
'
