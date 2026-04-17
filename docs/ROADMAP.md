# bunzo — roadmap

Milestones are deliberately narrow. Each one is a shippable, testable artifact either in QEMU or on real hardware.

## Phase 0 — Bootstrap

- [x] MIT license
- [x] Repo scaffolding: `README.md`, `docs/`, `.gitignore`
- [x] Build-system direction chosen: Buildroot, `BR2_EXTERNAL` pattern, Docker-wrapped, QEMU-first
- [x] Language decision: **Rust** for all bunzo-written code; WASM (via `wasmtime`) for skills
- [x] M1 scaffolding
- [x] M1 Target 1 complete — bunzo boots in QEMU and identifies as bunzo

## Milestone 1 — "Hello, bunzo" ✅

**Goal:** build the first bunzo image end-to-end from Linux kernel source + a from-source userland (no upstream distro), and boot it.

**Scaffolding to create**

- [x] `board/external.mk`, `board/external.desc`, `board/Config.in` — Buildroot `BR2_EXTERNAL` hooks
- [x] `board/bunzo/common/rootfs-overlay/etc/os-release` — `ID=bunzo`, `PRETTY_NAME="bunzo 0.0.1"`
- [x] `board/bunzo/common/rootfs-overlay/etc/motd` — bunzo banner
- [x] `board/bunzo/common/rootfs-overlay/etc/hostname` — `bunzo`
- [x] `board/bunzo/common/linux.fragment` — kernel-config additions (cgroups v2, namespaces, seccomp, audit, overlayfs, hardware RNG)
- [x] `board/bunzo/common/post-build.sh` — post-rootfs hook
- [x] `scripts/bootstrap.sh` — clone Buildroot at a pinned tag into `./buildroot/`
- [x] `scripts/build.sh` — wrap `make -C buildroot` with `BR2_EXTERNAL`
- [x] `scripts/run-qemu.sh` — boot the image in `qemu-system-aarch64 -M virt`
- [x] `Dockerfile.builder` + `scripts/build-docker.sh` — macOS-friendly build wrapper

**Target 1 (minimum) ✅**

- [x] `board/configs/bunzo_qemu_aarch64_defconfig`
- [x] Full build succeeds end-to-end in Docker on macOS
- [x] `./scripts/run-qemu.sh qemu_aarch64` boots the image
- [x] Banner shows on first screen; `cat /etc/os-release` shows `ID=bunzo`; `hostname` returns `bunzo`; `systemctl --version` confirms systemd is init

**Target 2 (stretch — real Pi hardware)**

- [ ] `board/configs/bunzo_rpi4_defconfig`
- [ ] Build produces `dist/bunzo-rpi4-0.0.1.img`
- [ ] Image boots on a real Pi 4 from SD
- [ ] Same banner / os-release / hostname on hardware

**Target 3 (stretch — generic PC)**

- [ ] `board/configs/bunzo_pc_x86_64_defconfig`
- [ ] Build produces `dist/bunzo-pc-x86_64-0.0.1.img` (or `.iso`)
- [ ] USB-flashed image boots on a UEFI laptop
- [ ] Same banner / os-release / hostname

**Definition of done (minimum):** `./scripts/build-docker.sh qemu_aarch64` builds from scratch, `./scripts/run-qemu.sh qemu_aarch64` boots it, and the running system identifies as bunzo. **Met on 2026-04-15.**

**Followups surfaced by the first boot:**

- systemd was built without seccomp, audit, or PAM support (`systemctl --version` → `-SECCOMP -AUDIT -PAM`). The kernel has these compiled in, but systemd can't apply them to services without the userland libs. Needs `BR2_PACKAGE_LIBSECCOMP=y` + the systemd feature toggles before M4 agent sandboxing.

## Milestone 2 — "Chat shell (stub)"

**Goal:** bunzo boots directly into a Rust-powered chat-like shell instead of a login prompt.

**Builder changes**

- [x] Extend `Dockerfile.builder` with `rustup` + the `aarch64-unknown-linux-musl` target (and later `x86_64-unknown-linux-musl`)
- [x] Add a `bunzo-cargo` Docker named volume for the Cargo registry/git/target caches, same pattern as `bunzo-output` / `bunzo-dl`
- [x] Add `gcc-aarch64-linux-gnu` to the builder image and to the remote Linux host's apt deps — without it cargo can't link an aarch64 binary on an x86_64 host

**Crate**

- [x] `rust/bunzo-shell/` Cargo crate — minimal Rust chat shell; current proven path is line-oriented serial mode, with a richer `ratatui` / `crossterm` path still scaffolded
- [x] Stub behavior: echoes user input back with a bunzo-style canned response (no LLM yet)
- [x] Reads its banner/version from `/etc/os-release` so the shell and the OS stay in sync

**Wiring into the image**

- [x] `scripts/build.sh` cargo-builds `bunzo-shell` before invoking Buildroot and stages the static musl binary into `board/bunzo/common/rootfs-overlay/usr/bin/bunzo-shell`
- [x] Manual `/usr/bin/bunzo-shell` works from the recovery console in serial mode on `ttyAMA0`
- [x] Boot lands directly in the styled serial-mode shell on `ttyAMA0`
- [x] Explicit recovery path: `bunzo.recovery` on the kernel cmdline swaps in `serial-getty@ttyAMA0` via mutually exclusive `ConditionKernelCommandLine=` on the two units (`scripts/run-qemu.sh --recovery`)
- [x] Fullscreen `ratatui` on the PL011 serial console is deferred (retained behind `BUNZO_SHELL_MODE=tui`); M2 ships the styled line-oriented serial shell
- [ ] Survives reboot and works identically in QEMU and on Pi 4 (QEMU side verified; Pi 4 board defconfig is still an M1 stretch target)

**Non-goals for M2**

- No LLM calls. No `bunzod`. No skills. The shell is fully self-contained.

**Definition of done:** fresh boot shows the Rust chat shell on the serial console, typing "hello" gets a bunzo-style stub response, an explicit recovery path exists, and `ps` shows `bunzo-shell` under 5 MB RSS.

**Status (2026-04-16):** first three boxes verified on the Debian remote builder; the RSS measurement is still pending an in-VM reading (binary is 594 KB stripped static musl, so the ceiling is safe but un-formally confirmed). Treating M2 as effectively complete and opening M3.

## Milestone 3 — "Actual agent"

**Goal:** the chat shell is backed by a real LLM via a Rust daemon.

- [ ] `rust/bunzod/` Cargo crate — Rust agent daemon on `tokio`
- [ ] Local Unix-socket API at `/run/bunzod.sock` (length-prefixed JSON request/response to start; can move to `postcard` once the shape is stable)
- [ ] `bunzo-shell` talks to `bunzod` over the socket; no direct LLM calls from the shell
- [ ] Pluggable backend behind a Rust trait, with two implementations:
  - Remote: `async-openai` (or equivalent) for easy prototyping
  - Local: `candle` or a `llama.cpp` FFI binding, added in parallel
- [ ] Append every exchange to an action-ledger file on disk (append-only JSONL to start)
- [ ] Skill registry scaffolding — empty, but hook points are in place for M4
- [ ] systemd unit for `bunzod` with socket activation

**Definition of done:** from the chat shell, the user asks "what time is it?", `bunzod` answers with system time via the configured backend, the ledger records the exchange, and `bunzod` idles under 10 MB RSS when no model is loaded.

## Milestone 4 — "First skill"

**Goal:** the agent can do something in the world through a sandboxed skill.

- [ ] Skill interface defined as a **WebAssembly module + manifest**: a `wasm32-wasi` binary exporting a narrow entry point, plus a TOML manifest declaring the capabilities the skill needs
- [ ] `bunzod` embeds `wasmtime` and exposes a capability-scoped host API (path-whitelisted file reads, timers, HTTP to explicit hosts, etc.)
- [ ] One real skill end-to-end, compiled to WASM: e.g. `set-reminder` or `read-local-file`
- [ ] Policy check before skill invocation; denial is the default
- [ ] Ledger records which skill ran, with what inputs, and the result
- [ ] Sandboxing comes from the `wasmtime` boundary itself — no `bwrap`/`nsjail`/seccomp juggling for the skill runner. (systemd-side seccomp/audit still matters for non-skill services and is handled separately.)

**Definition of done:** user asks for a reminder, the WASM skill fires inside `bunzod`, the reminder shows up on time, the ledger records it, and the skill only touches the resources its manifest declared.

## Milestone 5 — "Phone pairing"

**Goal:** talk to bunzo from a phone without a cloud round-trip.

Deliberately vague for now; concrete design once Milestones 1–4 land.

## Beyond

- Local model runtime baked into the image (llama.cpp / ollama)
- Read-only rootfs with writable overlay for state
- OTA updates, signed A/B partitions
- More boards (additional SBCs, more x86_64 variants)
- Policy-engine DSL in plain language
- Audit UI for reviewing historical agent actions
