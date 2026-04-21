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
- M8 scheduler slice 1

The next major milestone is M7 provisioning. M8 is started but not complete.

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

## Milestone 7 — Provisioning engine

**Goal:** replace the current `/setup` shortcut with a real provisioning
service that owns durable config, secrets, and first-boot state.

Open work:

- [ ] Introduce `bunzo-provisiond`
- [ ] Persist provisioning/config state under `/var/lib/bunzo/config/`
- [ ] Persist secrets under `/var/lib/bunzo/secrets/`
- [ ] Render runtime-facing files from canonical state instead of treating
  `/etc` as the source of truth
- [ ] Make `/setup` call the provisioning API instead of writing files directly
- [ ] Implement a restart-safe first-boot state machine
- [ ] Support both local-shell setup and headless phone/browser setup
- [ ] Validate provider credentials before declaring setup complete

**Definition of done:** a user can complete setup locally or from a phone, both
paths end in the same persisted config, and `/setup` is just one frontend to
the provisioning engine.

See [PROVISIONING.md](PROVISIONING.md) for the intended service boundary and
state machine.

## Milestone 8 — Scheduler

**Goal:** proactive jobs run through the same task, policy, and audit path as
interactive work.

Done in slice 1:

- [x] Durable job store
- [x] Time-based recurring interval triggers
- [x] Safe due-job claiming and job-run history
- [x] Normal `scheduled_job` task runs
- [x] Same runtime policy path as interactive work
- [x] Shell job commands
- [x] Persisted job-run failure state

Still open:

- [ ] Persist retry/backoff policy
- [ ] Add richer trigger shapes beyond fixed intervals
- [ ] Improve job editing/inspection surfaces as history grows

**Current status:** QEMU-verified. `/jobs every 10 what OS is this?` now
creates recurring work that pauses/resumes under the same approval-first policy
model as interactive requests.

## Milestone 9 — Phone control

**Goal:** after provisioning, a user can interact with bunzo from a phone
without dropping into a shell.

Open work:

- [ ] Local phone/browser client for an already-provisioned device
- [ ] Trust/pairing model
- [ ] Access to replies plus historical action review

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
