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

**Status (2026-04-16):** first three boxes verified on the Debian remote builder; the RSS measurement is still pending an in-VM reading (binary is 594 KB stripped static musl, so the ceiling is safe but un-formally confirmed). Infrastructure to take the measurement has landed alongside the M3 scaffold: `scripts/run-qemu.sh` now hostfwds `tcp::2222->:22`, `etc/ssh/sshd_config.d/bunzo-dev.conf` permits empty-password root (DEV-ONLY), and `scripts/measure-shell-rss.sh` greps `/proc/$pid/status` over SSH. Blocked only on booting the next image. Treating M2 as effectively complete and M3 as scaffolded.

## Milestone 3 — "Actual agent"

**Goal:** the chat shell is backed by a real LLM via a Rust daemon.

- [x] `rust/bunzod/` Cargo crate — Rust agent daemon on `tokio` (current_thread runtime)
- [x] Local Unix-socket API at `/run/bunzod.sock` (4-byte big-endian length prefix + JSON body, `bunzo-proto` v1; internally-tagged `ClientMessage` / `ServerMessage` unions, 1 MiB per-frame cap)
- [x] `bunzo-shell` talks to `bunzod` over the socket; no direct LLM calls from the shell
- [x] `bunzo-shell` handles missing backend config in-band: warns when setup is missing, supports `/setup`, accepts a pasted API key, writes `/etc/bunzo/bunzod.toml` + `/etc/bunzo/openai.key`, and retries the request without leaving the shell
- [x] Pluggable backend behind a Rust trait (`Backend::stream_complete`), first implementation:
  - Remote: `async-openai 0.27` with `stream(true)` — key loaded from the file at `api_key_path` in `/etc/bunzo/bunzod.toml`, never via process env
  - Local: `candle` / `llama.cpp` FFI is deferred to after M3 boot verification
- [x] Append every exchange to an action-ledger file on disk — JSONL at `/var/lib/bunzo/ledger.jsonl`, `O_APPEND` + `sync_data()` per write (`{ts_ms, conv_id, user, assistant, backend, latency_ms, finish_reason}`)
- [ ] Skill registry scaffolding — empty, but hook points are in place for M4
- [x] systemd unit for `bunzod` with socket activation (`Type=notify` + `listenfd`; `bunzod.service` has no `[Install]`, so `bunzod.socket` pulls it in on first connect — daemon idles cold)

**Definition of done:** from the chat shell, the user asks "what time is it?", `bunzod` answers with system time via the configured backend, the ledger records the exchange, and `bunzod` idles under 10 MB RSS when no model is loaded. **Chat pipe verified on image 2026-04-17** (real remote completions streamed over the socket protocol). Project policy is now GPT-5.4-family only, with `gpt-5.4-mini` as the current interactive default. RSS measurement still pending (needs either a working ssh drop-in or a recovery-console sample). Ledger line format verified by code; on-image inspection pending for the same reason.

## Milestone 4 — "First skill"

**Goal:** the agent can do something in the world through a sandboxed skill.

- [x] Skill interface defined as a **WebAssembly module + manifest**: a `wasm32-unknown-unknown` cdylib exporting `bunzo_alloc` / `bunzo_dealloc` / `run`, plus a TOML manifest (`name`, `description`, JSON-Schema `parameters`, `[capabilities] fs_read = [...]`). Shared ABI lives in the `bunzo-skill-abi` crate.
- [x] `bunzod` embeds `wasmtime` and exposes a capability-scoped host API. First two hosts: `bunzo_fs_read` (path-whitelisted against the manifest; entries ending in `/` are directory prefixes, otherwise exact match; `..` segments always denied) and `bunzo_log` (skill-side stderr echo). Per-invocation memory cap 32 MiB, fuel budget 10M.
- [x] One real skill end-to-end, compiled to WASM: **`read-local-file`**. Input `{ "path": "..." }`, returns `{ "path": "...", "content": "..." }`. Ships whitelisted read paths: `/etc/os-release`, `/etc/motd`, `/etc/hostname`, `/proc/{meminfo,uptime,loadavg}`, `/var/lib/bunzo/**`.
- [x] Policy check before skill invocation; denial is the default. The manifest *is* the policy for M4 — anything not listed in `[capabilities]` is denied, with a DENIED log line in bunzod stderr. A user-in-the-loop policy engine is deferred.
- [x] Ledger records which skill ran, with what inputs, and the result. Each exchange's JSONL entry now carries a `tool_calls: [{name, ok, latency_ms}]` array.
- [x] Sandboxing comes from the `wasmtime` boundary itself — no `bwrap`/`nsjail`/seccomp juggling for the skill runner. (systemd-side seccomp/audit still matters for non-skill services and is handled separately.)

**Definition of done:** user asks a question that requires reading a bunzo-device file (e.g. "what OS is this?"), the LLM calls `read-local-file`, the host reads the file through the capability allowlist, the content is fed back to the LLM, the LLM answers, and the ledger records it. **Tool-call pipeline verified on image 2026-04-17** — model invoked `read-local-file`, `ToolActivity` frames streamed to the shell, skill error was fed back to the model and it recovered gracefully. Final step (wasmtime actually executing the skill) was blocked by an `env` vs `bunzo` import-module mismatch; fix committed to `bunzo-skill-abi` (`#[link(wasm_import_module = "bunzo")]`), needs one more build + smoke test cycle to confirm.

## Milestone 5 — "Provisioning mode"

**Goal:** a non-technical user can flash bunzo onto a device, get it online, name it, and connect an AI provider whether the hardware is headless or has a local screen/keyboard.

See [PROVISIONING.md](PROVISIONING.md) for the concrete state machine and service split this milestone should implement.

- [ ] First-boot state machine: `unprovisioned` → `network_ready` → `provider_ready` → `ready`
- [ ] Headless setup path on supported devices:
  - [ ] Wi-Fi AP + captive portal as the primary v1 path
  - [ ] Ethernet fallback via `bunzo.local/setup`
  - [ ] Clear "factory reset / re-enter setup mode" path
- [ ] Local setup path on supported devices:
  - [ ] `bunzo-shell` or a local setup frontend can drive the same provisioning state machine
  - [ ] Local setup asks for the same core fields as the phone flow, not a separate ad hoc config path
- [ ] Phone/browser setup UI asks only:
  - [ ] device name
  - [ ] internet connection (Wi-Fi join if needed)
  - [ ] AI provider choice
  - [ ] auth method (API key first; richer login flows later)
- [ ] Setup writes the initial persisted config (device identity, network, provider credentials)
- [ ] bunzo verifies the connection and provider live before finishing setup
- [ ] After deterministic setup finishes, bunzo can perform follow-up system tasks itself (timezone confirmation, optional personalization, device checks)

**Definition of done:** a user can flash bunzo, power it on, and complete setup either from a phone (headless path) or locally on the device (desktop path), ending in the same reachable ready state with the same persisted config.

## Milestone 6 — "Phone control"

**Goal:** talk to bunzo from a phone after setup without a cloud round-trip.

- [ ] Local phone client or browser UI can connect to the already-provisioned device
- [ ] Mutual trust / pairing model exists
- [ ] Agent replies and ledger review are accessible from the phone

**Definition of done:** after first-boot provisioning, the user can pick up a phone, reach bunzo on the local network, and continue using it without touching a shell or monitor.

## Beyond

- Local model runtime baked into the image (llama.cpp / ollama)
- Read-only rootfs with writable overlay for state
- OTA updates, signed A/B partitions
- More boards (additional SBCs, more x86_64 variants)
- Policy-engine DSL in plain language
- Audit UI for reviewing historical agent actions
