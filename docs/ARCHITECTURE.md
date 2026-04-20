# bunzo — architecture

This document separates **what exists now** (or is actively being built) from **what the project aims for later**. It is meant to stay honest: if something is not built yet, it lives under "Later".

## Principles

1. **Built from source, not layered on a base distro.** bunzo is assembled from upstream Linux kernel source plus a userland we compile ourselves. There is no "underlying OS" that we inherit. Every byte on the target was chosen and built by bunzo.
2. **Agent-native from the base up.** Kernel config, init system, and filesystem layout are chosen with the agent layer in mind — cgroups v2, namespaces, seccomp, audit, eventually a read-only rootfs with overlayfs for state.
3. **Multi-architecture by design.** The same bunzo userland runs on aarch64 (Pi 4/5 + other arm64 boards) and x86_64 (generic UEFI PCs) from day one. A new target is a new defconfig, not a rewrite.
4. **QEMU-first dev loop.** Any change builds and boots in QEMU in seconds. Hardware flashes are for validation, not iteration.
5. **Boring base, interesting top.** The layers below the agent runtime stay conventional (Linux kernel, standard init, standard filesystems) so we can focus novelty on the agent layer, not on reinventing the plumbing.
6. **Screen-optional product, deterministic first boot.** The shipping UX must work with no local screen, but must also support a normal local shell/setup path on hardware that has a display and input devices. First-boot setup should be a simple, reliable provisioning flow, not an open-ended LLM conversation.
7. **One provisioning engine, multiple surfaces.** Phone-led setup, local shell setup, and any later local UI should all drive the same provisioning state machine and write the same persisted config.

## Layers

```
┌───────────────────────────────────────────────────────┐
│  User interaction                                     │
│   - provisioning UI (phone-led)   [Later]             │
│   - local setup shell / console   [Now: M2/M3]        │
│   - phone client                  [Later]             │
│   - voice I/O                     [Later]             │
├───────────────────────────────────────────────────────┤
│  Agent runtime                                        │
│   - bunzod (agent daemon)         [Now: M3]           │
│   - skill registry                [Now: M4]           │
│   - context / task store          [Next: M5]          │
│   - policy engine                 [Next: M6]          │
│   - action ledger                 [Now: M3/M4]        │
│   - provisioning API / state      [Next: M7]          │
│   - scheduler / jobs              [Next: M8]          │
├───────────────────────────────────────────────────────┤
│  System services                                      │
│   - init + service manager        [Now: M1]           │
│   - logging / audit               [Now: M1]           │
│   - networking / ssh              [Now: M1]           │
│   - first-boot AP / captive UI    [Next: M7]          │
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
- **TUI / serial shell:** `ratatui` + `crossterm` for richer terminals, plus a plain serial mode for QEMU and recovery. The shell is both a developer/recovery interface and a legitimate local setup/usage surface on desktop-class hardware. It is not the only consumer onboarding UX.
- **Current serial-shell reality:** on the QEMU / PL011 serial console, the proven interaction mode today is plain line-oriented stdin/stdout on `ttyAMA0`. The richer `ratatui` / `crossterm` fullscreen path is still in the codebase, but it is not yet reliable enough to be the boot-critical path there.
- **Async runtime:** `tokio` for `bunzod` and any other long-lived network-facing services. Keep it single-threaded where possible to minimize per-process overhead.
- **Skill runtime:** WebAssembly via `wasmtime` embedded in `bunzod`. Skills compile to `wasm32-unknown-unknown` and run inside the daemon with capability-scoped host functions. This replaces the earlier plan to juggle `bwrap`/`nsjail` + seccomp for skill isolation — the `wasmtime` boundary is the sandbox.
- **Not written in Rust:** the kernel, libc, systemd, coreutils, openssh, and everything else Buildroot assembles for us. We pick and configure those but write none of their code.

**Build workflow for Rust code:** Rust binaries are cross-compiled inside the `bunzo-builder` Docker image (which gains `rustup` + the target's musl triple in M2) and dropped into the rootfs via `board/bunzo/common/rootfs-overlay/usr/bin/`. Later, once we have more than a couple of binaries, we promote them to proper Buildroot packages using `cargo-package` infrastructure.

## Now (current repo)

- **Kernel:** Linux, latest LTS, built from kernel.org source via Buildroot. Configured with a small bunzo-specific fragment layered on top of Buildroot's per-target defaults: cgroups v2, namespaces, seccomp-BPF, audit, overlayfs, hardware RNG.
- **libc:** glibc. (For maximum compatibility with the C ecosystem Buildroot pulls in and with future `llama.cpp`/`candle` bindings. musl would fight us on some of those. Rust binaries are still cross-compiled to the `*-linux-musl` triple so they are statically linked and self-contained regardless of the system libc.)
- **Init / service manager:** **systemd**. Decided in the M1 scaffolding batch — rejected the "busybox init first, migrate in M2" path because we would have replaced it immediately for socket activation and service supervision. The 20–40 MB cost is acceptable for an agent OS.
- **Userland:** Buildroot-assembled minimal rootfs — coreutils, bash, openssh, networking tools, `ca-certificates`, `haveged`, `sudo`. No language runtime — bunzo's own code (M2+) is Rust, cross-compiled to static musl binaries and staged via the rootfs overlay.
- **Identity:** `/etc/os-release`, `/etc/motd`, `/etc/hostname` — set to bunzo.
- **Shell / local console:** `bunzo-shell` is in the image and currently provides the only implemented setup surface. It can now collect the API key in-band via `/setup`, but that path still writes config files directly and is explicitly a stopgap before `bunzo-provisiond`.
- **Agent runtime:** `bunzod` is present today as a socket-activated Rust daemon behind a local Unix-socket API. It can stream model replies, invoke skills, and append to the action ledger.
- **Action ledger:** append-only JSONL audit at `/var/lib/bunzo/ledger.jsonl`. This is currently the only durable runtime record and should be treated as an audit sink, not the future canonical runtime store.
- **Skills:** one real skill exists today (`read-local-file`), compiled to WASM and executed inside `wasmtime` with manifest-scoped capabilities. The current "policy" is just the manifest allowlist.

## Next platform phase

After M4, bunzo's next implementation phase is the runtime foundation layer
described in [FOUNDATIONS.md](FOUNDATIONS.md).

- **Context / task store (M5):** durable conversations, tasks, snapshots, and
  runtime events above the JSONL ledger.
- **Policy engine (M6):** user-centric, task-aware allow/deny/approval
  decisions that sit in front of tool use and proactive execution.
- **Provisioning engine (M7):** `bunzo-provisiond`, persisted config under
  `/var/lib/bunzo`, config rendering into `/etc`, and `/setup` as a real
  frontend instead of a file-writing shortcut.
- **Scheduler (M8):** durable proactive jobs that create normal task runs and
  flow through the same state and policy layers as interactive work.

## Later

- **Agent daemon (`bunzod`):** remains the main local runtime entry point, but should evolve from "stateless request broker with audit log" into "task-aware runtime over a durable state store". Local-first (`candle` or `llama.cpp` FFI) backends still sit behind the same Rust trait.
- **Chat shell:** remains as a local escape hatch, developer console, and first-class local interface on devices with displays and keyboards. It talks to `bunzod` over a local Unix socket and can still handle manual setup, but it should not be the only onboarding path for headless hardware.
  See [PROVISIONING.md](PROVISIONING.md) for the concrete state machine and persisted-config boundary.
- **Skills:** WebAssembly modules the agent can invoke with explicit capabilities. Each ships a manifest declaring what it needs. `bunzod` loads and runs them inside the embedded `wasmtime`; capability enforcement happens at the host function boundary, so we do not need a separate sandbox runner.
- **Phone control / pairing:** after provisioning, a phone app or browser client talks to `bunzod` over a mutually authenticated local channel first, with optional remote reachability later. No mandatory cloud round-trip.
- **Read-only rootfs with overlayfs for state:** so agents cannot trash the base system and updates are atomic.
- **A/B partitions + signed updates:** for safe OTA.

## Deferred decisions

Called out so we do not paint ourselves into a corner, but we are not solving them yet.

- **Update mechanism:** manual re-flash for now. Later: OTA with signed A/B images.
- **Secrets storage:** filesystem mode 600 for now. Later: TPM-backed where hardware allows.
- **Multi-user:** single user for now. Later: local identity backend.
- **Telemetry:** none for now. Later: opt-in, local-first, never blocking.
- **Architectures beyond aarch64 + x86_64:** armv7, i386, RISC-V — revisit once the project has traction and a real user asks for them.
