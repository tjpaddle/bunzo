# bunzo — foundations plan

This document defines the next platform phase after M4. The goal is to turn
the current "chat shell + daemon + tools + ledger" prototype into a runtime
that can safely hold state, enforce policy, onboard itself deterministically,
and run proactive jobs.

## Why this order

The next four workstreams should land in this order:

1. **Context and task store**
2. **Policy engine**
3. **Provisioning engine**
4. **Job / scheduler layer**

That order is deliberate.

- The context/task store must come first because policy, provisioning, and
  scheduled work all need durable runtime state.
- Policy must come before real autonomy. Otherwise scheduled jobs and richer
  tools will outrun the trust model.
- Provisioning should be built on the same durable state and policy boundary
  instead of writing ad hoc files.
- The scheduler should come after state + policy exist, because proactive work
  is only useful if it is resumable, auditable, and bounded.

Do **not** chase multi-agent coordination before these four land. Right now
the missing pieces are not "more agents"; they are durable state, authority,
and deterministic system ownership.

## Foundation decisions

These are the planning assumptions for the next phase.

- **Canonical runtime state should move into a bunzo-owned store under
  `/var/lib/bunzo/state/`.**
- **The JSONL ledger stays as an append-only audit sink, not the primary source
  of truth.**
- **Frontends stay thin.** `bunzo-shell`, the future phone/browser setup UI,
  and later phone-control clients should call services, not write config files
  directly.
- **Provisioning and scheduling are deterministic services first, LLM-assisted
  second.**
- **Default-deny stays intact.** More convenience is acceptable only if the
  policy boundary gets stronger at the same time.

## Workstream 1 — context and task store

### Goal

Replace today's stateless request handling plus JSONL audit with durable
runtime state:

- resumable conversations
- task state
- context snapshots
- task/event history
- stable identifiers for future scheduler and phone clients

### Suggested shape

Use one local durable store as the canonical runtime database. The expected
default is SQLite unless a concrete constraint disproves it later.

Suggested first tables / records:

- `conversations`
- `messages`
- `tasks`
- `task_runs`
- `task_snapshots`
- `artifacts`
- `events`

Suggested semantics:

- A **conversation** is the user-visible thread.
- A **task** is the durable unit of work that can outlive one request.
- A **task run** is one execution attempt.
- A **snapshot** stores the resumable working state needed to continue after a
  reboot, disconnect, or tool round-trip.
- An **event** is the internal append-only record that can later feed both the
  JSONL ledger and richer UIs.

### First slice

The first implementation slice should be intentionally narrow:

1. `bunzod` creates a conversation + task for each shell session request.
2. Messages are written to the store before and after backend execution.
3. Tool invocations become first-class task events.
4. The shell gains a way to list and resume recent conversations/tasks.

### Exit criteria

- Rebooting the VM does not lose the current conversation/task history.
- A user can resume a recent thread without replaying raw JSONL.
- The runtime can store "task is waiting", "task completed", and "task failed"
  as durable state instead of inferring that from logs.

## Workstream 2 — policy engine

### Goal

Move from "the manifest is the policy" to a real user-centric policy layer
that evaluates actions in task context.

### Scope

The policy engine should evaluate at least:

- skill invocation
- filesystem access classes
- networked/external actions
- destructive actions
- scheduled/proactive actions
- provisioning actions that touch persistent config or secrets

### Suggested model

Start with structured rules, not natural-language policy authoring.

Suggested concepts:

- **Subject:** user request, provisioning flow, scheduler, or system task
- **Action:** invoke skill, read path, write path, call provider, mutate config
- **Resource:** skill name, path prefix, provider, secret class, config area
- **Decision:** allow, deny, or require approval
- **Grant scope:** once, task, session, persistent

Persist policy state in bunzo-owned runtime data, not in shell-local memory.

### First slice

1. Keep manifests as the capability ceiling.
2. Add a policy evaluator in front of skill invocation.
3. Add durable grants/denials with audit records.
4. Surface denials and approvals in the shell as first-class runtime events.

### Exit criteria

- Every tool action is evaluated against both manifest capability and runtime
  policy.
- Decisions are durable and reviewable.
- The runtime can distinguish "allowed by default", "allowed because user
  granted it", and "denied".

## Workstream 3 — provisioning engine

### Goal

Make `/setup` a frontend to a real provisioning service instead of a shortcut
that writes `/etc/bunzo/*` directly.

### Components

- `bunzo-provisiond`
- persisted provisioning/config state under `/var/lib/bunzo/config/`
- secret storage under `/var/lib/bunzo/secrets/`
- a renderer / activator for runtime-facing files under `/etc/`
- local-shell setup frontend
- headless phone/browser frontend

### First slice

Implement provisioning in two slices, but under one milestone:

1. **Engine + local frontend**
   - own the provisioning state machine
   - persist device/network/provider state under `/var/lib/bunzo`
   - render runtime-facing config
   - make `/setup` call the provisioning API instead of writing files
2. **Headless surface**
   - temporary AP + captive portal
   - `bunzo-setup-ui`
   - Ethernet fallback via `bunzo.local/setup`

### Exit criteria

- `/setup` no longer writes ad hoc config files directly.
- Local and headless setup write the same persisted config model.
- Provisioning state survives reboot and can resume cleanly.
- `/etc/bunzo/bunzod.toml` is rendered from canonical state, not treated as the
  source of truth.

## Workstream 4 — job / scheduler layer

### Goal

Add proactive routines only after state and policy exist.

### Components

- durable job definitions
- trigger specification
- due-job claiming / leasing
- run history
- retry / backoff rules
- task creation hooks into the context/task store
- policy checks before execution

### Suggested shape

Keep scheduler ownership separate from the shell. The likely first shape is a
dedicated local service, `bunzo-schedulerd`, that reads the shared bunzo state
store, claims due jobs, and creates task runs through the same runtime APIs the
shell uses.

This keeps proactive execution explicit and avoids smuggling "always-on"
behavior into a daemon that is currently designed around socket-activated
interactive requests.

### First slice

1. Repeating local jobs only
2. Time-based triggers only
3. Each fired job creates a normal task run
4. The same policy engine gates execution
5. Results show up in the same task/event history as interactive work

### Exit criteria

- bunzo can run a routine such as "check X every morning" without custom glue.
- Each scheduled run is resumable, auditable, and policy-bounded.
- Scheduler failures do not disappear into logs; they appear as task/job state.

## Milestone order

The roadmap after M4 should follow this sequence:

1. **M5 — Context and task store**
2. **M6 — Policy engine**
3. **M7 — Provisioning engine**
4. **M8 — Scheduler**
5. **M9 — Phone control and richer remote clients**

## What comes after

Once these foundations land, bunzo is in a much better position to add:

- phone control and review UX
- richer local and remote clients
- local-model routing
- plain-language policy authoring
- multi-agent / delegation work

Without these foundations, those later features would be built on top of
ephemeral state and implicit trust, which is exactly the wrong order.
