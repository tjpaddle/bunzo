# bunzo — roadmap

Load this file when you need milestone status or the current open work. It is
no longer intended to be a full historical log.

## Current phase

The QEMU development loop has completed the core runtime foundations:

- M1 bootable image
- M2 local shell
- M3 agent daemon
- M4 first skill
- M5 durable runtime store
- M6 runtime policy
- M7 provisioning engine
- M8 scheduler

The current milestone focus can move to M9 phone control and later hardware
replay. M7 and M8 are closed in the QEMU development loop, with remaining
Wi-Fi AP/captive-portal and hardware-radio validation tracked as future
connectivity hardening rather than blockers for the provisioning engine
milestone. M9 browser control is now QEMU-verified for the current browser
surface: the existing socket-activated HTTP frontend becomes a paired browser
control surface after provisioning reaches `ready`, and post-setup actions,
history summaries, conversation detail, task detail, approvals, and tool
resume still go through `bunzod`.

## Completed milestones

- **M1 — Hello, bunzo**
  Buildroot-based bunzo image boots in QEMU and identifies as bunzo.
- **M2 — Chat shell**
  `bunzo-shell` boots as the local shell and stays within the memory budget in
  QEMU.
- **M3 — Actual agent**
  `bunzod` streams model replies over a local Unix socket and records audit
  data.
- **M4 — First skill**
  `read-local-file` runs in `wasmtime` and is verified on-image.
- **M5 — Context and task store**
  Runtime state moved into SQLite with durable conversations, tasks, task runs,
  snapshots, and events.
- **M6 — Policy engine**
  Runtime policy is durable, shell-authorable, approval-capable, and enforced
  in front of tool use.
- **M7 — Provisioning engine**
  `bunzo-provisiond` owns durable config, secrets, setup state, runtime
  rendering, and restart/boot reconciliation under `/var/lib/bunzo/` for local
  shell and headless HTTP setup.
- **M8 — Scheduler**
  Proactive jobs are durable and run through the same task, policy, approval,
  and audit path as interactive work.

## Milestone 7 — Provisioning engine

**Goal:** replace the current `/setup` shortcut with a real provisioning
service that owns durable config, secrets, and first-boot state.

Closed work:

- [x] Introduce `bunzo-provisiond`
- [x] Persist provisioning/config state under `/var/lib/bunzo/config/`
- [x] Persist secrets under `/var/lib/bunzo/secrets/`
- [x] Render runtime-facing files from canonical state instead of treating
  `/etc` as the source of truth
- [x] Make `/setup` call the provisioning API instead of writing files directly
- [x] Implement a restart-safe first-boot state machine for the local-shell
  path
- [x] Reconcile rendered runtime config from canonical `/var/lib/bunzo/` state
  on restart/boot
- [x] Validate provider credentials before declaring setup complete
- [x] Add the headless phone/browser setup frontend
- [x] Make device name a real provisioning-owned hostname and reconcile it on
  restart/boot
- [x] Make the current `existing_network` path render and reconcile explicit
  runtime-facing network config from canonical provisioning state
- [x] Make the current `existing_network` path take an explicit interface name
  instead of assuming `eth0`
- [x] Add at least one additional connectivity mode beyond the current
  explicit-interface `existing_network` slice boundary
- [x] Add Wi-Fi client provisioning groundwork with canonical WPA state,
  rendered WPA runtime config, and on-image WPA tooling

**Current status:** Complete and QEMU-verified. `bunzo-provisiond` persists
canonical state under `/var/lib/bunzo/`, `/setup` talks to its socket API,
provider credentials are live-validated before `ready`, canonical provisioning
state re-renders `/etc/hostname`, `/etc/network/interfaces`,
`/etc/wpa_supplicant/wpa_supplicant.conf`, and `/etc/bunzo/bunzod.toml` on
restart/boot, and `bunzo-setup-httpd` exposes the same setup/status flow over
the QEMU/dev network path. Device name is a real live + persistent hostname,
both frontends carry explicit `existing_network`, `static_ipv4`, and
`wifi_client` fields, and connectivity state remains canonical under
`/var/lib/bunzo/config/network.toml`.

**Definition of done:** a user can complete setup locally or from a phone, both
paths end in the same persisted config, and `/setup` is just one frontend to
the provisioning engine.

See [PROVISIONING.md](PROVISIONING.md) for the intended service boundary and
state machine.

## Milestone 8 — Scheduler

**Goal:** proactive jobs run through the same task, policy, and audit path as
interactive work.

Done so far:

- [x] Durable job store
- [x] Time-based recurring interval triggers
- [x] One-shot delayed triggers
- [x] Daily triggers
- [x] Safe due-job claiming and job-run history
- [x] Normal `scheduled_job` task runs
- [x] Same runtime policy path as interactive work
- [x] Shell job commands
- [x] Persisted job-run failure state
- [x] Persist retry/backoff policy
- [x] Bounded retry claims for failed interval runs
- [x] Shell retry/backoff flags and pending retry display
- [x] Shell one-shot command and completed one-shot display
- [x] Shell job details, pause/resume, and edit commands

Closed work:

- [x] Add additional trigger shapes beyond fixed intervals and one-shot delays
- [x] Improve job editing/inspection surfaces as history grows

**Current status:** Complete and QEMU-verified. `/jobs every`, `/jobs in`, and
`/jobs daily` create interval, one-shot, and daily work through the same
scheduler/runtime/task/policy path. Jobs persist retry/backoff policy, record
trigger kind and attempt number, show pending retry/error state in
`/jobs list`, and clear pending retry state when deleted. `/jobs show`,
`/jobs pause`, `/jobs resume`, and `/jobs edit` provide the inspection and
editing surface needed for the current durable scheduler.

## Milestone 9 — Phone control

**Goal:** after provisioning, a user can interact with bunzo from a phone
without dropping into a shell.

Open work:

- [x] Local phone/browser client for an already-provisioned device
- [x] Trust/pairing model
- [x] Access to replies plus historical action review

Done so far:

- [x] Reuse `bunzo-setup-httpd` as the already-provisioned browser surface
      instead of adding a second runtime path
- [x] Browser message send through the existing `bunzod` Unix-socket protocol
- [x] Browser JSON endpoints for recent conversations, recent tasks, and
      scheduled jobs
- [x] Browser approval resolution for waiting task runs through the existing
      approval/resume path
- [x] Browser-control pairing gate with local pairing code, hashed durable
      trust state, HTTP-only session cookie, and post-ready `/setup` gating
- [x] Browser conversation detail endpoint/view for stored user and assistant
      replies
- [x] Browser task detail endpoint/view for policy decisions, waits, tool
      invocation/results, and completion history

**Current status:** complete and QEMU-verified for the current browser-control
scope. After setup is `ready`, unpaired browsers see a pairing page and
control APIs return `401 pairing_required`. Pairing uses a local code under
`/var/lib/bunzo/control/pairing-code`, stores hashed durable trust/session
material under `/var/lib/bunzo/control/trust.toml`, and sets an HTTP-only
browser session cookie. Paired control UI and `/api/*` endpoints send
messages, list runtime history summaries, list jobs, show conversation replies
and task action history, surface waiting approvals, and resume approved tasks
through the same `bunzod` task/policy/audit path used by `bunzo-shell` and
`bunzo-schedulerd`.

## Hardware/stretch targets

Still open:

- [ ] `bunzo_rpi4` real hardware target
- [ ] `bunzo_pc_x86_64` generic PC target
- [ ] Replay the post-M5 runtime smoke path on hardware

## Later

- Local model runtime
- Read-only rootfs with writable state overlay
- OTA and A/B updates
- Plain-language policy authoring
- Audit/review UI
- Multi-agent/delegation work only after the remaining foundations are real
