# bunzo — architecture

Load this file when you need the current runtime/service shape. It is meant to
answer "what exists now, what owns what, and what is still missing?"

## Core shape

- **Build system:** Buildroot with a `BR2_EXTERNAL` tree under `board/`
- **Primary target:** `qemu_aarch64`
- **Default dev loop:** edit locally, build on the remote Linux host, boot QEMU
  there over SSH
- **Init system:** systemd
- **bunzo-written code:** Rust
- **Skill runtime:** WebAssembly in `wasmtime`
- **Canonical runtime state:** SQLite under `/var/lib/bunzo/state/`

## Runtime services

### `bunzo-shell`

The local shell and current local control surface.

Current commands include:

- `/setup`
- `/conversations`
- `/tasks`
- `/policy`
- `/approve`
- `/approvals`
- `/jobs`

Today it is still both a developer console and the temporary local setup path.

### `bunzod`

The main agent runtime daemon.

Responsibilities:

- local Unix-socket API
- model interaction
- tool/skill execution
- conversation/task/task-run/event persistence
- runtime policy evaluation in front of tool use
- waiting/resume flow for approval-gated work

`bunzod` is still the main runtime entry point, but it is no longer treated as
a stateless request broker. The real source of truth is the runtime store.

### `bunzo-schedulerd`

The scheduler/job service introduced in M8 slice 1.

Responsibilities:

- read durable jobs from the shared runtime store
- claim due work under lease
- create normal `scheduled_job` task runs
- execute them through the same prepared-request/runtime path as shell work

Important constraint: scheduler work must keep sharing the main runtime/task/
policy path. There should not be a second scheduler-only execution pipeline.

### `bunzo-provisiond` (next)

Not built yet. This should become the owner of first-boot and reconfiguration
state so `/setup` stops writing `/etc/bunzo/*` directly.

## Key data paths

- Runtime state:
  `/var/lib/bunzo/state/runtime.sqlite3`
- Audit sink:
  `/var/lib/bunzo/ledger.jsonl`
- Current runtime config:
  `/etc/bunzo/bunzod.toml`
- Current backend secret:
  `/etc/bunzo/openai.key`

Today `/etc/bunzo/*` is still directly written by the shell. The architectural
target is for provisioning-owned state under `/var/lib/bunzo/` to become the
source of truth and `/etc` to become rendered runtime output.

## Current runtime model

### Interactive path

`bunzo-shell` request → `bunzod` → runtime store/task creation → policy
evaluation → skill/model execution → task events/state updates

### Scheduler path

`bunzo-schedulerd` claim due job → prepare `scheduled_job` request → same
runtime/task/policy path as interactive work

### Policy model

- Skill manifest = hard capability ceiling
- Runtime policy = durable allow/deny/require-approval layer
- Default unmatched tool use = `require_approval` / `once`

## Product surfaces

### Exists now

- local shell on the device
- local runtime store and audit trail
- proactive interval jobs via `/jobs`

### Next

- real provisioning engine with local and headless frontends
- scheduler hardening beyond interval-only slice 1

### Later

- phone/browser control after provisioning
- read-only rootfs + durable writable state
- OTA/update machinery

## Stable project decisions

- bunzo is built from source, not layered on another distro
- the product is screen-optional, not phone-only
- frontends should stay thin and call services
- provisioning and scheduling should be deterministic services first,
  LLM-assisted second
