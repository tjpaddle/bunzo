# bunzo — provisioning

Load this file only for M7 provisioning work.

## Current gap

Today `/setup` is still a shell-owned shortcut that writes `/etc/bunzo/*`
directly. That is useful for development, but it is not the long-term product
shape and it is not compatible with durable config ownership.

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

## What setup should ask

Keep the v1 flow minimal:

1. device name
2. connectivity method
3. AI provider
4. credentials

That is enough to get the device into a useful ready state.

## Frontends

### Local setup

For devices with local I/O:

- `bunzo-shell` detects unprovisioned state
- `/setup` enters the provisioning flow
- local setup asks for the same core fields as the phone/browser flow

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

## Handoff into normal runtime

When provisioning reaches `ready`:

1. persist config and secrets
2. render runtime-facing config
3. stop provisioning-only services
4. start the normal runtime mode
5. mark provisioning complete

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
