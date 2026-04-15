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
- **Builds run inside Docker** so the workflow is macOS-friendly. The `Dockerfile.builder` image has all Buildroot build dependencies; `scripts/build-docker.sh` mounts the repo into the container and runs `scripts/build.sh` inside. Heavy write paths (`output/`, `dl/`) go through Docker named volumes, not the macOS virtiofs bind mount, because virtiofs takes SIGBUS under Buildroot's mmap-heavy writes.
- **Output per target:** kernel image + rootfs + bootable artifact (`.img` for SD/USB, raw files for QEMU). Targets:
  - `bunzo_qemu_aarch64` — QEMU arm64 virt machine, for fast dev iteration
  - `bunzo_rpi4` — real Pi 4 / Pi 5 boot
  - `bunzo_pc_x86_64` — generic x86_64 UEFI PC (USB-flashable)

## Languages

bunzo's own code is written in **Rust**. This is a foundational decision driven by the project's "extremely minimal RAM" constraint and the desire to keep the baseline footprint low on small boards.

- **Why Rust over Python:** no interpreter, no GC, no runtime baggage. A `tokio`-based service idles at 5–10 MB RSS; a tight service can sit under 5 MB. Python would add ~30 MB to the rootfs plus per-process interpreter overhead.
- **Why Rust over Go:** Go's runtime adds a 2–5 MB floor per process plus GC overhead. Rust gives a cleaner single-static-binary story on embedded targets and lets us share buffers with C libraries (`llama.cpp` etc.) at zero cost.
- **TUI:** `ratatui` + `crossterm`. The chat shell on `tty1` is a Rust TUI, not a Python script or a shell loop.
- **Async runtime:** `tokio` for `bunzod` and any other long-lived network-facing services. Keep it single-threaded where possible to minimize per-process overhead.
- **Skill runtime:** WebAssembly via `wasmtime` embedded in `bunzod`. Skills compile to `wasm32-wasi` and run inside the daemon with capability-scoped host functions. This replaces the earlier plan to juggle `bwrap`/`nsjail` + seccomp for skill isolation — the `wasmtime` boundary is the sandbox.
- **Not written in Rust:** the kernel, libc, systemd, coreutils, openssh, and everything else Buildroot assembles for us. We pick and configure those but write none of their code.

**Build workflow for Rust code:** Rust binaries are cross-compiled inside the `bunzo-builder` Docker image (which gains `rustup` + the target's musl triple in M2) and dropped into the rootfs via `board/bunzo/common/rootfs-overlay/usr/bin/`. Later, once we have more than a couple of binaries, we promote them to proper Buildroot packages using `cargo-package` infrastructure.

## Now (M1 scope)

- **Kernel:** Linux, latest LTS, built from kernel.org source via Buildroot. Configured with a small bunzo-specific fragment layered on top of Buildroot's per-target defaults: cgroups v2, namespaces, seccomp-BPF, audit, overlayfs, hardware RNG.
- **libc:** glibc. (For maximum compatibility with the C ecosystem Buildroot pulls in and with future `llama.cpp`/`candle` bindings. musl would fight us on some of those. Rust binaries are still cross-compiled to the `*-linux-musl` triple so they are statically linked and self-contained regardless of the system libc.)
- **Init / service manager:** **systemd**. Decided in the M1 scaffolding batch — rejected the "busybox init first, migrate in M2" path because we would have replaced it immediately for socket activation and service supervision. The 20–40 MB cost is acceptable for an agent OS.
- **Userland:** Buildroot-assembled minimal rootfs — coreutils, bash, openssh, networking tools, `ca-certificates`, `haveged`, `sudo`. No language runtime — bunzo's own code (M2+) is Rust, cross-compiled to static musl binaries and staged via the rootfs overlay.
- **Identity:** `/etc/os-release`, `/etc/motd`, `/etc/hostname` — set to bunzo.
- **No agent runtime yet.** M1 exists to prove the from-source build-boot loop, not to land the agent layer.

## Later

- **Agent daemon (`bunzod`):** long-lived Rust service on `tokio`. Brokers LLM calls (`async-openai` or similar for remote; a `candle` or `llama.cpp` FFI binding for local inference), holds the action ledger, enforces policy, embeds `wasmtime` for skill execution, exposes a local Unix-socket API for the chat shell and other clients. Target footprint: idle RSS under 10 MB (excluding loaded model weights).
- **Chat shell:** minimal Rust TUI (`ratatui` + `crossterm`) pinned to `tty1` on boot, replacing the default getty. Talks to `bunzod` over a local Unix socket. Ctrl-Alt-F2 stays as an escape hatch. Target footprint: idle RSS under 5 MB.
- **Skills:** WebAssembly modules the agent can invoke with explicit capabilities. Each ships a manifest declaring what it needs. `bunzod` loads and runs them inside the embedded `wasmtime`; capability enforcement happens at the host function boundary, so we do not need a separate sandbox runner.
- **Policy engine:** rules the user sets (eventually in plain language) that `bunzod` enforces before invoking any skill. Denial is the default.
- **Action ledger:** append-only log of every agent action — inputs, outputs, policy decision, timing. Reviewable via the chat shell or a phone client.
- **Phone pairing:** phone app talks to `bunzod` over a mutually authenticated channel. QR pairing + WireGuard tunnel. No cloud round-trip.
- **Model backend:** local-first (`candle` or `llama.cpp` FFI), remote as an optional fallback. Swappable behind a Rust trait.
- **Read-only rootfs with overlayfs for state:** so agents cannot trash the base system and updates are atomic.
- **A/B partitions + signed updates:** for safe OTA.

## Deferred decisions

Called out so we do not paint ourselves into a corner, but we are not solving them yet.

- **Update mechanism:** manual re-flash for now. Later: OTA with signed A/B images.
- **Secrets storage:** filesystem mode 600 for now. Later: TPM-backed where hardware allows.
- **Multi-user:** single user for now. Later: local identity backend.
- **Telemetry:** none for now. Later: opt-in, local-first, never blocking.
- **Architectures beyond aarch64 + x86_64:** armv7, i386, RISC-V — revisit once the project has traction and a real user asks for them.
