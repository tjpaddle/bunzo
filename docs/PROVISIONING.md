# bunzo — provisioning

Load this file only for M7 provisioning work.

## Current slice

The first two local-shell-focused M7 slices are now landed:

- `bunzo-provisiond` exists as the provisioning owner
- local-shell `/setup` calls the provisioning socket API instead of writing
  `/etc/bunzo/*` directly
- canonical config/secrets/state live under `/var/lib/bunzo/`
- `/etc/bunzo/bunzod.toml` is rendered runtime output, not source of truth
- bogus OpenAI credentials do not transition provisioning to `ready`
- restart/boot reconciliation re-renders `/etc/bunzo/bunzod.toml` from
  canonical `/var/lib/bunzo/` state

Still open:

- headless phone/browser frontend
- hostname/network activation beyond the local-shell defaults used in this
  first slice

## Required outcome

Provisioning needs one engine and multiple frontends:

- **engine:** `bunzo-provisiond`
- **local frontend:** `bunzo-shell`
- **headless frontend:** phone/browser setup UI

All frontends must write the same durable config model and drive the same
restart-safe state machine.

## Core rules

- provisioning state/config lives under `/var/lib/bunzo/`
- secrets live under `/var/lib/bunzo/secrets/`
- `/etc` is rendered runtime output, not the source of truth
- setup is deterministic system-owned work, not open-ended LLM orchestration
- the AI can help only after the device is named, online, and provider-ready

## Minimal v1 state machine

1. `unprovisioned`
2. `naming`
3. `connectivity`
4. `provider`
5. `validating`
6. `ready`
7. `failed_recoverable`

Transitions should survive reboot. Power loss during setup should resume from
saved state instead of restarting from zero.

In the current local-shell slices:

- `validating` performs a live OpenAI credential/model probe before `ready`
- failed provider validation lands in `failed_recoverable` with persisted
  detail for the frontend
- restart/boot reconciliation re-renders `/etc/bunzo/bunzod.toml` from
  canonical state before normal runtime use

## What setup should ask

Keep the v1 flow minimal:

1. device name
2. connectivity method
3. AI provider
4. credentials

That is enough to get the device into a useful ready state.

In the current local-shell slice, the engine seeds:

- device name from the current hostname unless a frontend overrides it
- connectivity as `existing_network`
- provider as OpenAI with the current GPT-5.4-family restriction

That keeps the state machine and durable ownership real while leaving richer
frontend UX for the next slice.

## Frontends

### Local setup

For devices with local I/O:

- `bunzo-shell` detects unprovisioned state
- `/setup` enters the provisioning flow
- the current slice persists current hostname/current network defaults and
  collects the provider credential locally

### Headless setup

For screenless devices:

- first boot enters provisioning mode
- device exposes a phone/browser setup path
- user completes the same core flow there

The exact AP/captive-portal stack is an implementation detail. The important
architectural rule is shared engine, shared state, shared resulting config.

## Durable ownership

Suggested durable layout:

- `/var/lib/bunzo/config/device.toml`
- `/var/lib/bunzo/config/network.toml`
- `/var/lib/bunzo/config/provider.toml`
- `/var/lib/bunzo/secrets/<provider>.key`

Then a renderer/activation step materializes runtime-facing files such as:

- `/etc/bunzo/bunzod.toml`
- hostname/network runtime config

The current slice renders only `/etc/bunzo/bunzod.toml`. Hostname/network
activation still remains future work.

Boot-time reconciliation is currently handled by
`bunzo-provisioning-reconcile.service`, while `bunzo-provisiond` and `bunzod`
also re-check canonical state on startup for defense in depth.

## Handoff into normal runtime

When provisioning reaches `ready`:

1. persist config and secrets
2. render runtime-facing config
3. stop provisioning-only services
4. start the normal runtime mode
5. mark provisioning complete

In the current local-shell slice, step 3 is effectively a no-op because
`bunzo-provisiond` is socket-activated and `bunzod` re-reads its config on
every request. The important boundary is still preserved: provisioning owns the
canonical state and renders the runtime-facing config.

## Re-entering setup

There must always be an explicit way to re-enter provisioning:

- local shell command
- phone/browser action
- hardware-specific reset path when supported

This should not require reflashing the image.

## Recommended implementation order

1. Define the durable config/secrets boundary under `/var/lib/bunzo/`
2. Introduce `bunzo-provisiond` with the state machine only
3. Make `/setup` call the provisioning API instead of writing files directly
4. Land the local-shell path first
5. Add the headless phone/browser path

Status:

- Steps 1 through 4 are now live for the local-shell/OpenAI path, including
  live provider validation and boot-time runtime-config reconciliation
- Step 5 is still open
