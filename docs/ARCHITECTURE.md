# bunzo — architecture

This document separates **what exists now** (or is actively being built) from **what the project aims for later**. It is meant to stay honest: if something is not built yet, it lives under "Later".

## Principles

1. **Built from source, not layered on a base distro.** bunzo is assembled from upstream Linux kernel source plus a userland we compile ourselves. There is no "underlying OS" that we inherit. Every byte on the target was chosen and built by bunzo.
2. **Agent-native from the base up.** Kernel config, init system, and filesystem layout are chosen with the agent layer in mind — cgroups v2, namespaces, seccomp, audit, eventually a read-only rootfs with overlayfs for state.
3. **Multi-architecture by design.** The same bunzo userland runs on aarch64 (Pi 4/5 + other arm64 boards) and x86_64 (generic UEFI PCs) from day one. A new target is a new defconfig, not a rewrite.
4. **QEMU-first dev loop.** Any change builds and boots in QEMU in seconds. Hardware flashes are for validation, not iteration.
5. **Boring base, interesting top.** The layers below the agent runtime stay conventional (Linux kernel, standard init, standard filesystems) so we can focus novelty on the agent layer, not on reinventing the plumbing.

## Layers

```
┌───────────────────────────────────────────────────────┐
│  User interaction                                     │
│   - chat shell on tty1            [Later]             │
│   - phone client                  [Later]             │
│   - voice I/O                     [Later]             │
├───────────────────────────────────────────────────────┤
│  Agent runtime                                        │
│   - bunzod (agent daemon)         [Later]             │
│   - skill registry                [Later]             │
│   - policy engine                 [Later]             │
│   - action ledger                 [Later]             │
├───────────────────────────────────────────────────────┤
│  System services                                      │
│   - init + service manager        [Now: M1]           │
│   - logging / audit               [Now: M1]           │
│   - secret / key storage          [Later]             │
├───────────────────────────────────────────────────────┤
│  Userland                                             │
│   - libc, coreutils, shell        [Now: M1]           │
│   - network stack, openssh        [Now: M1]           │
│   - Python runtime                [Now: M2]           │
├───────────────────────────────────────────────────────┤
│  Kernel                                               │
│   - Linux (upstream, our config)  [Now: M1]           │
├───────────────────────────────────────────────────────┤
│  Firmware / bootloader                                │
│   - U-Boot / Pi firmware / UEFI   [Now: M1]           │
└───────────────────────────────────────────────────────┘
```

## Build system

- **Framework:** Buildroot. We do not vendor it. `scripts/bootstrap.sh` clones a pinned release tag into `./buildroot/` (gitignored).
- **Layout:** `BR2_EXTERNAL` pattern. Our configurations live under `board/` in this repo; Buildroot is invoked with `BR2_EXTERNAL=$(pwd)/board` so our tree is the source of truth for kernel config, rootfs overlay, defconfigs, and custom packages.
- **Builds run inside Docker** so the workflow is macOS-friendly. The `Dockerfile.builder` image has all Buildroot build dependencies; `scripts/build-docker.sh` mounts the repo into the container and runs `scripts/build.sh` inside.
- **Output per target:** kernel image + rootfs + bootable artifact (`.img` for SD/USB, raw files for QEMU). Targets:
  - `bunzo_qemu_aarch64` — QEMU arm64 virt machine, for fast dev iteration
  - `bunzo_rpi4` — real Pi 4 / Pi 5 boot
  - `bunzo_pc_x86_64` — generic x86_64 UEFI PC (USB-flashable)

## Now (M1 scope)

- **Kernel:** Linux, latest LTS, built from kernel.org source via Buildroot. Configured with a small bunzo-specific fragment layered on top of Buildroot's per-target defaults: cgroups v2, namespaces, seccomp-BPF, audit, overlayfs, hardware RNG.
- **libc:** glibc. (For maximum compatibility — we will want Python, ML libs, and arbitrary C extensions later. musl would fight us there.)
- **Init / service manager:** decided in the M1 scaffolding batch. Leaning busybox init to start (tiny, fast boot) and migrating to systemd in M2 when the chat shell needs real service supervision and socket activation.
- **Userland:** Buildroot-assembled minimal rootfs — busybox or coreutils, bash, openssh, networking tools, ca-certificates. Python comes in M2.
- **Identity:** `/etc/os-release`, `/etc/motd`, `/etc/hostname` — set to bunzo.
- **No agent runtime yet.** M1 exists to prove the from-source build-boot loop, not to land the agent layer.

## Later

- **Agent daemon (`bunzod`):** long-lived user-space process. Brokers LLM calls, holds the action ledger, enforces policy, exposes a local Unix-socket API for the chat shell and other clients.
- **Chat shell:** minimal TUI pinned to `tty1` on boot, replacing the default getty. Talks to `bunzod` over a local Unix socket. Ctrl-Alt-F2 stays as an escape hatch.
- **Skills:** small sandboxed programs the agent can invoke with explicit capabilities. Each ships a manifest declaring what it needs. Sandbox eventually via `bwrap`/`nsjail` + seccomp + cgroups.
- **Policy engine:** rules the user sets (eventually in plain language) that `bunzod` enforces before invoking any skill. Denial is the default.
- **Action ledger:** append-only log of every agent action — inputs, outputs, policy decision, timing. Reviewable via the chat shell or a phone client.
- **Phone pairing:** phone app talks to `bunzod` over a mutually authenticated channel. QR pairing + WireGuard tunnel. No cloud round-trip.
- **Model backend:** local-first (llama.cpp / ollama), remote as an optional fallback. Swappable.
- **Read-only rootfs with overlayfs for state:** so agents cannot trash the base system and updates are atomic.
- **A/B partitions + signed updates:** for safe OTA.

## Deferred decisions

Called out so we do not paint ourselves into a corner, but we are not solving them yet.

- **Update mechanism:** manual re-flash for now. Later: OTA with signed A/B images.
- **Secrets storage:** filesystem mode 600 for now. Later: TPM-backed where hardware allows.
- **Multi-user:** single user for now. Later: local identity backend.
- **Telemetry:** none for now. Later: opt-in, local-first, never blocking.
- **Architectures beyond aarch64 + x86_64:** armv7, i386, RISC-V — revisit once the project has traction and a real user asks for them.
