# bunzo — provisioning

Load this file only for M7 provisioning work.

## Current slice

The first three M7 provisioning slices are now landed:

- `bunzo-provisiond` exists as the provisioning owner
- local-shell `/setup` calls the provisioning socket API instead of writing
  `/etc/bunzo/*` directly
- canonical config/secrets/state live under `/var/lib/bunzo/`
- `/etc/bunzo/bunzod.toml` is rendered runtime output, not source of truth
- bogus OpenAI credentials do not transition provisioning to `ready`
- restart/boot reconciliation re-renders `/etc/bunzo/bunzod.toml` from
  canonical `/var/lib/bunzo/` state
- `bunzo-setup-httpd` now exposes the same setup/status flow over HTTP on the
  current dev/QEMU network path
- device name is now a real live + persistent system hostname owned by
  canonical provisioning state
- the current `existing_network` path now renders and reconciles
  `/etc/network/interfaces` from canonical provisioning state

Still open:

- broader connectivity beyond the current `existing_network` slice

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

In the current landed slices:

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

In the current landed slice set, the engine seeds:

- device name from the current hostname unless a frontend overrides it, then
  applies it as the live + persistent system hostname
- connectivity as `existing_network`, rendered into the current
  `/etc/network/interfaces` runtime output
- provider as OpenAI with the current GPT-5.4-family restriction

That keeps the state machine and durable ownership real while leaving broader
connectivity work for the next slice.

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

Current implementation:

- `bunzo-setup-httpd` is the thin HTTP frontend
- it talks to `bunzo-provisiond` over the provisioning socket API
- it shows status plus submits device name, `existing_network`, OpenAI, and
  the API key
- in the QEMU/dev loop it is reachable on guest port `8080` and forwarded from
  host port `8080`

The exact AP/captive-portal stack remains future work. The important
architectural rule is already preserved: shared engine, shared state, shared
resulting config.

## Durable ownership

Suggested durable layout:

- `/var/lib/bunzo/config/device.toml`
- `/var/lib/bunzo/config/network.toml`
- `/var/lib/bunzo/config/provider.toml`
- `/var/lib/bunzo/secrets/<provider>.key`

Then a renderer/activation step materializes runtime-facing files such as:

- `/etc/hostname`
- `/etc/network/interfaces`
- `/etc/bunzo/bunzod.toml`

The current slice now renders all three of those runtime-facing outputs, but
the connectivity side is still intentionally narrow to the `existing_network`
path.

Boot-time reconciliation is currently handled by
`bunzo-provisioning-reconcile.service`, while `bunzo-provisiond` and `bunzod`
also re-check canonical state on startup for defense in depth. That
reconciliation now re-applies the runtime hostname, the current
`existing_network` interfaces file, and `/etc/bunzo/bunzod.toml`.

## Handoff into normal runtime

When provisioning reaches `ready`:

1. persist config and secrets
2. render runtime-facing config
3. stop provisioning-only services
4. start the normal runtime mode
5. mark provisioning complete

In the current slice, step 3 is effectively a no-op because
`bunzo-provisiond`, `bunzo-setup-httpd`, and `bunzod` are all socket- or
request-activated enough that setup can stay thin while the provisioning
boundary remains preserved: provisioning owns the canonical state and renders
the runtime-facing config.

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

- Steps 1 through 5 are now live for the current OpenAI path, including
  live provider validation, boot-time runtime hostname/network/config
  reconciliation, and the first browser-accessible headless frontend
- The remaining provisioning work is broader connectivity beyond
  `existing_network`
