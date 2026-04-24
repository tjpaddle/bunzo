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

It is still the developer console, and `/setup` is now the first local
frontend for `bunzo-provisiond` rather than a direct file-writing shortcut.

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

### `bunzo-provisiond`

The provisioning service introduced in the first M7 slice.

Responsibilities:

- own durable provisioning state under `/var/lib/bunzo/provisioning/`
- own canonical config under `/var/lib/bunzo/config/`
- own canonical secrets under `/var/lib/bunzo/secrets/`
- render `/etc/hostname` from canonical device state
- render `/etc/network/interfaces` from canonical connectivity state for the
  current explicit-interface `existing_network` slice
- render `/etc/bunzo/bunzod.toml` from canonical provisioning state
- apply the live system hostname from canonical device state
- validate provider credentials before provisioning reaches `ready`
- reconcile rendered runtime hostname/network/config outputs from canonical
  state on restart/boot
- expose a local Unix-socket API that thin frontends such as `bunzo-shell`
  and `bunzo-setup-httpd` call during setup/reconfiguration

Current scope covers both the local-shell path and the first headless browser
path. Connectivity activation is still intentionally narrow to the
explicit-interface `existing_network` path.

### `bunzo-setup-httpd`

The thin headless provisioning frontend introduced in the latest M7 slice.

Responsibilities:

- expose a minimal browser/HTTP setup surface on the dev network path
- show provisioning status from `bunzo-provisiond`
- submit the same canonical setup inputs through the provisioning socket API
- avoid owning provisioning logic or writing `/var/lib/bunzo/` or `/etc/bunzo/`
  directly

Current shape: socket-activated HTTP on guest port `8080`. In the QEMU dev
loop, `run-qemu.sh` forwards host port `8080` to that guest port.

## Key data paths

- Runtime state:
  `/var/lib/bunzo/state/runtime.sqlite3`
- Audit sink:
  `/var/lib/bunzo/ledger.jsonl`
- Provisioning state:
  `/var/lib/bunzo/provisioning/state.toml`
- Canonical device config:
  `/var/lib/bunzo/config/device.toml`
- Canonical connectivity config:
  `/var/lib/bunzo/config/network.toml`
- Canonical provider config:
  `/var/lib/bunzo/config/provider.toml`
- Canonical backend secret:
  `/var/lib/bunzo/secrets/openai.key`
- Current runtime hostname:
  `/etc/hostname`
- Current runtime connectivity config:
  `/etc/network/interfaces`
- Current runtime config:
  `/etc/bunzo/bunzod.toml`

`/etc/hostname`, `/etc/network/interfaces`, and `/etc/bunzo/bunzod.toml` are
rendered runtime outputs. The source of truth for setup lives under
`/var/lib/bunzo/`. In the current slice, `network.toml` owns both the
`existing_network` kind and its explicit interface name.

## Current runtime model

### Interactive path

`bunzo-shell` request → `bunzod` → runtime store/task creation → policy
evaluation → skill/model execution → task events/state updates

### Provisioning path

`bunzo-shell /setup` or `bunzo-setup-httpd` → `bunzo-provisiond` → canonical
`/var/lib/bunzo/` state/config/secrets → rendered `/etc/hostname`,
`/etc/network/interfaces` for the chosen interface, and
`/etc/bunzo/bunzod.toml` → live hostname application + provider validation →
normal `bunzod` request path on the next shell request

### Provisioning reconciliation path

boot/startup → `bunzo-provisioning-reconcile.service` and startup
reconciliation hooks → canonical `/var/lib/bunzo/` state → re-rendered
`/etc/hostname`, `/etc/network/interfaces`, and `/etc/bunzo/bunzod.toml`

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
- provisioning via `bunzo-provisiond` with both local-shell and headless HTTP
  frontends
- boot-safe runtime hostname/network/config reconciliation from canonical
  provisioning state

### Next

- additional connectivity modes beyond the current explicit-interface
  `existing_network` slice
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
