# bunzo — foundations

This file explains the ordering of the platform work. It is mostly planning
rationale, not day-to-day implementation detail.

## Why the order matters

The project needed to land the foundations in this order:

1. durable runtime state
2. runtime policy
3. provisioning as a real service
4. proactive scheduling

That order still matters because:

- provisioning and scheduling both need durable state
- proactive work is unsafe without a real policy boundary
- `/etc` should not keep accumulating ad hoc ownership
- multi-agent features are premature until these foundations are stable

## Current result

- **M5 is done in the QEMU dev loop.**
  Runtime state now lives in SQLite and survives reboot.
- **M6 is done in the QEMU dev loop.**
  Runtime policy is durable, shell-authorable, and approval-capable.
- **M8 slice 1 is done in the QEMU dev loop.**
  `bunzo-schedulerd` exists and scheduled work uses the same task/policy path
  as interactive work.

## What remains

### Provisioning

The biggest remaining foundations gap is M7:

- durable provisioning/config ownership under `/var/lib/bunzo/`
- `bunzo-provisiond`
- `/setup` as a frontend, not a file writer
- one setup state machine shared by local and headless flows

### Scheduler hardening

M8 exists, but not in final form:

- retry/backoff policy is still missing
- only fixed interval triggers exist today
- job editing/history surfaces are still minimal

### Hardware replay

The post-M5 runtime path is verified in QEMU, not yet replayed on actual
hardware.

## Rules to preserve

- Keep the JSONL ledger as an audit/export sink, not the primary runtime store.
- Keep frontends thin.
- Keep manifests as the hard capability ceiling.
- Keep provisioning and scheduling deterministic by default.
- Do not prioritize multi-agent work before the remaining foundation gaps are
  closed.
