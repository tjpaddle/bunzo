# bunzo — roadmap

Milestones are deliberately narrow. Each one is a shippable, testable artifact either in QEMU or on real hardware.

## Phase 0 — Bootstrap

- [x] MIT license
- [x] Repo scaffolding: `README.md`, `docs/`, `.gitignore`
- [x] Build-system direction chosen: Buildroot, `BR2_EXTERNAL` pattern, Docker-wrapped, QEMU-first
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

**Goal:** bunzo boots directly into a chat-like shell instead of a login prompt.

- [ ] A simple TUI launched as a systemd unit on `tty1`, replacing getty
- [ ] Echoes user input back with a bunzo-style response (no LLM yet)
- [ ] Ctrl-Alt-F2 still gives a normal shell as an escape hatch
- [ ] Survives reboot and works identically in QEMU and on Pi 4
- [ ] Python runtime included in the rootfs (we need it for the stub and for M3)

**Definition of done:** fresh boot shows the chat shell, not a login prompt; typing "hello" gets a response; escape hatch works.

## Milestone 3 — "Actual agent"

**Goal:** the chat shell is backed by a real LLM.

- [ ] `bunzod` daemon (Python to start) with a local Unix-socket API
- [ ] Chat shell talks to `bunzod`, not to an LLM directly
- [ ] Pluggable backend: remote API first (easy to prototype), local model (llama.cpp) added in parallel
- [ ] Append every exchange to an action-ledger file on disk
- [ ] Skill registry scaffolding (even if it starts empty)

**Definition of done:** user says "what time is it?", agent answers with system time, ledger records the exchange.

## Milestone 4 — "First skill"

**Goal:** the agent can do something in the world through a sandboxed skill.

- [ ] Skill interface defined (tiny — one input, one output, capability manifest)
- [ ] One real skill end-to-end: e.g. `set-reminder` or `read-local-file` with an explicit path whitelist
- [ ] Policy check before skill invocation; denial is the default
- [ ] Sandbox primitives in place: seccomp + namespaces + cgroups
- [ ] Ledger records which skill ran, with what inputs, and the result

**Definition of done:** user asks for a reminder, skill fires, reminder shows up on time, ledger records it.

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
