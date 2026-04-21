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

**Status (2026-04-20):** verified on the running `qemu_aarch64` guest. In-guest `/proc/$pid/status` sampling over the forwarded SSH port measured `bunzo-shell` at **572 kB VmRSS**, comfortably inside the M2 budget. QEMU-side reboot persistence is also now confirmed for the runtime-state work layered on top of the shell.

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
- [x] Skill registry scaffolding landed and is now populated by M4 skills
- [x] systemd unit for `bunzod` with socket activation (`Type=notify` + `listenfd`; `bunzod.service` has no `[Install]`, so `bunzod.socket` pulls it in on first connect — daemon idles cold)

**Definition of done:** from the chat shell, the user asks "what time is it?", `bunzod` answers with system time via the configured backend, the ledger records the exchange, and `bunzod` idles under 10 MB RSS when no model is loaded. **Chat pipe verified on image 2026-04-17** (real remote completions streamed over the socket protocol). Project policy is now GPT-5.4-family only, with `gpt-5.4-mini` as the current interactive default. Idle-RSS is now measured on-image at **6932 kB VmRSS** on `qemu_aarch64`, which clears the memory budget. Ledger line format verified by code; optional on-image ledger inspection still remains if we want a fully explicit operational close-out note.

## Milestone 4 — "First skill"

**Goal:** the agent can do something in the world through a sandboxed skill.

- [x] Skill interface defined as a **WebAssembly module + manifest**: a `wasm32-unknown-unknown` cdylib exporting `bunzo_alloc` / `bunzo_dealloc` / `run`, plus a TOML manifest (`name`, `description`, JSON-Schema `parameters`, `[capabilities] fs_read = [...]`). Shared ABI lives in the `bunzo-skill-abi` crate.
- [x] `bunzod` embeds `wasmtime` and exposes a capability-scoped host API. First two hosts: `bunzo_fs_read` (path-whitelisted against the manifest; entries ending in `/` are directory prefixes, otherwise exact match; `..` segments always denied) and `bunzo_log` (skill-side stderr echo). Per-invocation memory cap 32 MiB, fuel budget 10M.
- [x] One real skill end-to-end, compiled to WASM: **`read-local-file`**. Input `{ "path": "..." }`, returns `{ "path": "...", "content": "..." }`. Ships whitelisted read paths: `/etc/os-release`, `/etc/motd`, `/etc/hostname`, `/proc/{meminfo,uptime,loadavg}`, `/var/lib/bunzo/**`.
- [x] Policy check before skill invocation; denial is the default. The manifest *is* the policy for M4 — anything not listed in `[capabilities]` is denied, with a DENIED log line in bunzod stderr. A user-in-the-loop policy engine is deferred.
- [x] Ledger records which skill ran, with what inputs, and the result. Each exchange's JSONL entry now carries a `tool_calls: [{name, ok, latency_ms}]` array.
- [x] Sandboxing comes from the `wasmtime` boundary itself — no `bwrap`/`nsjail`/seccomp juggling for the skill runner. (systemd-side seccomp/audit still matters for non-skill services and is handled separately.)

**Definition of done:** user asks a question that requires reading a bunzo-device file (e.g. "what OS is this?"), the LLM calls `read-local-file`, the host reads the file through the capability allowlist, the content is fed back to the LLM, the LLM answers, and the ledger records it.

**Status (2026-04-21):** operationally closed for the QEMU development loop. The tool-call pipeline is verified on-image end to end: `read-local-file` executes inside wasmtime, `ToolActivity` frames stream to the shell, and the assistant can answer from device-local file contents. The later M6 policy smoke also re-confirmed the same path under runtime policy control: explicit runtime `deny` blocked the tool, and removing that denial restored the normal skill path while leaving the manifest as the hard capability ceiling.

## Next phase — runtime foundations

After M4, bunzo should stop widening the surface area and start hardening the
runtime. The next four milestones are intentionally ordered:

1. durable context/task state
2. policy
3. provisioning as a real service
4. proactive scheduling

See [FOUNDATIONS.md](FOUNDATIONS.md) for the cross-cutting plan and the
dependency rationale.

## Milestone 5 — "Context and task store"

**Goal:** replace "current request + JSONL audit" with durable runtime state
that can resume after reboot, power loss, or reconnect.

- [x] Introduce a canonical bunzo-owned runtime state store under
  `/var/lib/bunzo/state/`
- [x] Persist conversations and message history as first-class runtime objects
- [x] Persist tasks and task-run state (`queued`, `running`, `waiting`,
  `completed`, `failed`)
- [x] Store resumable context snapshots instead of reconstructing state from the
  shell or from logs
- [x] Record tool activity as structured task events, not only as audit text
- [x] Keep the JSONL ledger as an append-only audit/export sink
- [x] Add shell commands to list and resume recent conversations/tasks

**Definition of done:** a conversation survives reboot, the user can resume it
without replaying raw ledger lines, and `bunzod` can answer "what tasks exist
and what state are they in?" from durable state.

**Status (2026-04-20):** closed for the QEMU development loop. The runtime store now lives under `/var/lib/bunzo/state/`, `bunzo-shell` exposes `/conversations` and `/tasks`, explicit `waiting`/`completed` paths are visible from durable state, and QEMU reboot persistence is verified. Real-hardware replay of the same persistence/waiting-path smoke is still worth doing, but it is now deferred follow-up work rather than the blocker for starting M6.

## Milestone 6 — "Policy engine"

**Goal:** replace manifest-only gating with a user-centric policy layer that
decides whether bunzo may perform an action in the context of a task.

- [x] Introduce policy concepts: subject, action, resource, decision, scope
- [x] Keep manifest capabilities as the hard ceiling for skill behavior
- [x] Add a policy evaluator in front of skill invocation
- [x] Persist grants and denials as runtime state
- [x] Support at least `allow`, `deny`, and `require approval`
- [x] Surface policy decisions and denials in the shell and audit trail
- [x] Apply the same evaluator to future scheduler-triggered work

**Definition of done:** every tool action is checked against both manifest
capability and runtime policy, decisions are durable/reviewable, and bunzo can
distinguish "explicitly allowed" from "denied by default".

**Status (2026-04-21):** operationally closed in QEMU for both the interactive
shell path and the first scheduler-created task path. `bunzod` persists durable
`runtime_policies` in the SQLite runtime store, evaluates runtime policy in
front of skill invocation, records `policy.decision` events on tasks/task-runs,
and `bunzo-shell` exposes `/policy list`, `/policy allow`, `/policy deny`,
`/policy require-approval`, `/policy delete`, `/approve`, and `/approvals`.
On-image smoke has covered all four interactive policy branches: persistent
`deny` blocks `read-local-file`, `require_approval` leaves the task in durable
`waiting`, approval resolution resumes the same waiting task-run at `once` /
`task` / `session` / `persistent`, and the unmatched-tool default now pauses on
`require_approval` / `once` instead of implicitly allowing the tool. The first
M8 scheduler slice now uses the same evaluator/default posture via the
`scheduled_job` subject and task kind.

## Milestone 7 — "Provisioning engine"

**Goal:** a non-technical user can flash bunzo onto a device, get it online, name it, and connect an AI provider whether the hardware is headless or has a local screen/keyboard.

See [PROVISIONING.md](PROVISIONING.md) for the concrete state machine and service split this milestone should implement.

- [ ] Introduce `bunzo-provisiond` as the owner of the provisioning state
  machine
- [ ] Persist setup state and config under `/var/lib/bunzo/config/`
- [ ] Persist secrets under `/var/lib/bunzo/secrets/`
- [ ] Render runtime-facing files (`/etc/bunzo/bunzod.toml`, hostname, network
  config) from canonical state
- [ ] Make `/setup` call the provisioning API instead of writing files directly
- [ ] First-boot state machine: `unprovisioned` → `naming` → `connectivity` → `provider` → `validating` → `ready`
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

**Definition of done:** `/setup` is just one frontend to `bunzo-provisiond`, a
user can complete setup either from a phone (headless path) or locally on the
device (desktop path), and both surfaces end in the same persisted config.

## Milestone 8 — "Scheduler"

**Goal:** bunzo can run proactive routines through the same task, policy, and
audit path as interactive requests.

- [x] Introduce a durable job store
- [x] Support time-based recurring triggers
- [x] Claim due jobs safely and record job-run history
- [x] Create normal task runs for scheduler-fired work
- [x] Run scheduler-triggered work through the same policy engine
- [x] Persist job-run failure state
- [ ] Persist retries/backoff policy
- [x] Expose job status and recent runs in the shell

**Definition of done:** bunzo can run a routine such as "check X every
morning", each run is recorded as a normal task, and scheduled actions are
resumable, auditable, and policy-bounded.

**Status (2026-04-21):** the first scheduler slice landed and is QEMU-verified.
`bunzo-schedulerd` now exists as a dedicated service, the runtime store has
durable `scheduled_jobs` and `scheduled_job_runs`, `/jobs` in `bunzo-shell` can
create/list/delete interval jobs, and each firing creates a normal
`scheduled_job` task run through the existing runtime/task/policy path. On
image, `/jobs every 10 what OS is this?` created a recurring job whose first
run paused on the default `require_approval` / `once` posture, `/approve latest
persistent` resumed that same waiting run, and later recurring runs completed
normally through the same path. Remaining M8 work is richer trigger shapes and
persisted retry/backoff policy.

## Milestone 9 — "Phone control"

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
- Multi-agent / delegation work, but only after the state/policy/scheduler
  foundations are real
