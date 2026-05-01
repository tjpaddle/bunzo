# bunzo - hardware replay

This runbook is for replaying the post-M5 runtime, provisioning, scheduler,
and browser-control smoke paths on real hardware. It is deliberately a replay
path, not a new runtime path: provisioning state remains canonical under
`/var/lib/bunzo/`, browser control stays on `bunzo-setup-httpd`, and scheduled
work still goes through `bunzo-schedulerd` plus the shared runtime task/policy
store.

## Current target

The first concrete replay target is `cm5_nano_a`: Raspberry Pi Compute Module
5 on a Waveshare CM5-NANO-A carrier.

Use wired Ethernet and the current `existing_network` provisioning mode for
the first replay. Wi-Fi client state can be checked after basic replay, but
AP/captive-portal bring-up and hardware-radio validation are later
connectivity hardening work.

## Build and artifact handoff

Build pushed `main` on the remote Linux builder:

```sh
./scripts/remote-build.sh cm5_nano_a
```

The hardware artifact is:

```text
output/cm5_nano_a/images/sdcard.img
```

Fetch it to the local machine when the flasher is local:

```sh
./scripts/remote-fetch-image.sh cm5_nano_a
```

Use `--dry-run` to verify that the remote image exists and record its SHA-256
without copying the image:

```sh
./scripts/remote-fetch-image.sh cm5_nano_a --dry-run
```

## Flash

Put the CM5-NANO-A into eMMC flashing mode with the BOOT button, attach USB-C,
power the board, and run `rpiboot` on the flasher host. Then write
`dist/cm5_nano_a/sdcard.img` to the enumerated block device.

Double-check the block device before writing. On macOS, the fast raw device is
usually `/dev/rdiskN`; on Linux, it is usually `/dev/sdX` or `/dev/mmcblkX`.

Example macOS shape:

```sh
diskutil list
diskutil unmountDisk /dev/diskN
sudo dd if=dist/cm5_nano_a/sdcard.img of=/dev/rdiskN bs=4m status=progress conv=sync
sync
diskutil eject /dev/diskN
```

Example Linux shape:

```sh
lsblk
sudo umount /dev/sdX? 2>/dev/null || true
sudo dd if=dist/cm5_nano_a/sdcard.img of=/dev/sdX bs=4M status=progress conv=fsync
sync
```

## Boot and discover

Connect Ethernet before first boot. Optional serial fallback is the CM5 debug
UART at `ttyAMA10`, 115200 baud, as configured by
`board/bunzo/cm5_nano_a/cmdline.txt`.

Find the device IP from the DHCP server, serial console, or local network
scan. Set it locally for the rest of the runbook:

```sh
export BUNZO_HW_HOST=192.168.1.50
```

Basic pre-setup checks:

```sh
curl -fsS "http://${BUNZO_HW_HOST}:8080/status"
ssh root@"${BUNZO_HW_HOST}" 'systemctl --failed --no-pager'
```

The current dev image permits empty-password root SSH. That is a development
posture only and is not a shipping security posture.

## Provisioning smoke

Use the browser setup form at:

```text
http://<device-ip>:8080/setup
```

For the first replay, use:

- device name: a hardware-specific name such as `bunzo-cm5-1`
- connectivity: `existing_network`
- interface: `eth0`
- provider: OpenAI, using a GPT-5.4-family model accepted by the form

Do not paste API keys into shared logs. Prefer the browser form for the
provider key. If using scripted setup, pass the key through a private
environment or stdin path and unset it immediately afterward.

After setup:

```sh
curl -fsS "http://${BUNZO_HW_HOST}:8080/status"
ssh root@"${BUNZO_HW_HOST}" 'hostname; cat /etc/hostname; sed -n "1,80p" /etc/network/interfaces'
ssh root@"${BUNZO_HW_HOST}" 'test -s /var/lib/bunzo/provisioning/state.toml'
ssh root@"${BUNZO_HW_HOST}" 'test -s /var/lib/bunzo/config/provider.toml'
ssh root@"${BUNZO_HW_HOST}" 'test -s /var/lib/bunzo/config/network.toml'
```

Do not print files under `/var/lib/bunzo/secrets/`.

## Browser-control smoke

Before pairing, control APIs should be locked:

```sh
curl -i "http://${BUNZO_HW_HOST}:8080/api/bootstrap"
```

Expected result: `401` with `pairing_required`.

Open the root page in a browser:

```text
http://<device-ip>:8080/
```

Read the local pairing code only from the hardware or a private SSH session:

```sh
ssh root@"${BUNZO_HW_HOST}" 'cat /var/lib/bunzo/control/pairing-code'
```

Pair in the browser, then use the browser UI to send:

```text
Return exactly HARDWARE_BROWSER_OK
```

Expected result: the assistant replies with `HARDWARE_BROWSER_OK`, and the
browser can show recent conversations, tasks, jobs, conversation detail, and
task detail.

For the approval path, ask the browser to read `/etc/os-release`. Confirm that
the task reaches waiting approval for
`read-local-file:fs-read:/etc/os-release`, approve once from the browser, and
verify the completed task detail includes the policy decision, tool
invocation, tool result, and assistant completion.

## Scheduler smoke

The CM5 serial getty uses `ttyAMA10`, while `bunzo-shell.service` currently
only owns `ttyAMA0` when that device exists. For hardware replay, run
`bunzo-shell` manually over SSH in serial mode:

```sh
ssh -tt root@"${BUNZO_HW_HOST}" 'env BUNZO_SHELL_MODE=serial bunzo-shell'
```

Inside the shell:

```text
/jobs in 10 Return exactly HARDWARE_SCHEDULER_OK
/jobs list
```

Wait for the one-shot job to run, then inspect:

```text
/jobs list
/tasks
```

Expected result: the one-shot job disables itself after completion, the run is
recorded as `scheduled_job`, and the task uses the same policy/audit path as
interactive work.

## Final on-device checks

Run these before calling the hardware replay done:

```sh
ssh root@"${BUNZO_HW_HOST}" 'systemctl --failed --no-pager'
ssh root@"${BUNZO_HW_HOST}" 'systemctl is-active bunzo-provisiond.socket bunzo-setup-http.socket bunzod.socket bunzo-schedulerd.service'
ssh root@"${BUNZO_HW_HOST}" 'test -s /var/lib/bunzo/state/runtime.sqlite3'
ssh root@"${BUNZO_HW_HOST}" 'find /var/lib/bunzo -maxdepth 2 -type d -exec ls -ld {} +'
```

Completion requires all of the following on actual CM5-NANO-A hardware:

- boot reaches multi-user mode
- provisioning reaches `ready` through `bunzo-provisiond`
- canonical state is under `/var/lib/bunzo/`
- rendered `/etc` outputs match the selected provisioning state
- browser pairing and browser message smoke pass
- approval/resume smoke passes through the existing runtime policy path
- scheduler one-shot smoke passes through the existing runtime task path
- no failed systemd units remain unexplained

An image build alone is not hardware replay completion.
