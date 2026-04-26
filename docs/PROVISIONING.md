# bunzo — provisioning

Load this file only for M7 provisioning work.

## Current status

M7 provisioning is now landed and QEMU-verified:

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
- the current `existing_network` path now persists an explicit interface name
  in canonical state instead of assuming `eth0`
- `static_ipv4` now persists interface/address/prefix/gateway/DNS under
  `/var/lib/bunzo/config/network.toml` and renders a static ifupdown stanza
  into `/etc/network/interfaces`
- `wifi_client` now persists interface/SSID/key-secret metadata under
  `/var/lib/bunzo/config/network.toml`, stores the WPA passphrase under
  `/var/lib/bunzo/secrets/wifi-psk`, renders
  `/etc/wpa_supplicant/wpa_supplicant.conf`, and renders
  `/etc/network/interfaces` with a `wpa-conf` stanza
- the QEMU image includes WPA client tooling (`wpa_supplicant`, `iw`, and
  wireless regulatory data)

Real AP/captive-portal bring-up and hardware-radio validation remain later
connectivity hardening. They are not a separate provisioning engine boundary:
any future frontend must still call `bunzo-provisiond` and write the same
canonical `/var/lib/bunzo/` state.

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
- restart/boot reconciliation re-renders runtime outputs from canonical state
  before normal runtime use

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
- connectivity as `existing_network` with an explicit interface name,
  defaulting to the current canonical value or `eth0` when a frontend does not
  override it, rendered into the current `/etc/network/interfaces` runtime
  output
- connectivity can alternatively be `static_ipv4`, with interface,
  address/prefix, optional gateway, and optional DNS servers persisted in the
  same canonical `network.toml` and rendered into `/etc/network/interfaces`
- connectivity can alternatively be `wifi_client`, with interface, SSID, and
  WPA passphrase persisted through canonical `network.toml` plus
  `/var/lib/bunzo/secrets/wifi-psk`, rendered into
  `/etc/network/interfaces` and `/etc/wpa_supplicant/wpa_supplicant.conf`
- provider as OpenAI with the current GPT-5.4-family restriction

That keeps the state machine and durable ownership real while leaving AP and
captive-portal UX for later connectivity hardening.

## Frontends

### Local setup

For devices with local I/O:

- `bunzo-shell` detects unprovisioned state
- `/setup` enters the provisioning flow
- the current implementation persists device name plus explicit
  `existing_network`, `static_ipv4`, or `wifi_client` fields, then collects
  the provider credential locally

### Headless setup

For screenless devices:

- first boot enters provisioning mode
- device exposes a phone/browser setup path
- user completes the same core flow there

Current implementation:

- `bunzo-setup-httpd` is the thin HTTP frontend
- it talks to `bunzo-provisiond` over the provisioning socket API
- it shows status plus submits device name, `existing_network`,
  `static_ipv4`, or `wifi_client`, the mode-specific network fields, OpenAI,
  and the API key
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
- `/var/lib/bunzo/secrets/wifi-psk` when `wifi_client` is configured

Then a renderer/activation step materializes runtime-facing files such as:

- `/etc/hostname`
- `/etc/network/interfaces`
- `/etc/wpa_supplicant/wpa_supplicant.conf`
- `/etc/bunzo/bunzod.toml`

The current implementation renders those runtime-facing outputs.
Connectivity is intentionally still ifupdown-based, but it now covers
explicit-interface DHCP through `existing_network`, static IPv4 through
`static_ipv4`, and WPA client setup through `wifi_client`.

Boot-time reconciliation is currently handled by
`bunzo-provisioning-reconcile.service`, while `bunzo-provisiond` and `bunzod`
also re-check canonical state on startup for defense in depth. That
reconciliation now re-applies the runtime hostname, the current
network interfaces file for the chosen connectivity mode, WPA client config
when applicable, and `/etc/bunzo/bunzod.toml`.

## Handoff into normal runtime

When provisioning reaches `ready`:

1. persist config and secrets
2. render runtime-facing config
3. stop provisioning-only services
4. start the normal runtime mode
5. mark provisioning complete

In the current implementation, step 3 is effectively a no-op because
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
  reconciliation, the first browser-accessible headless frontend, explicit
  `existing_network` interface ownership across shell, HTTP, status, and
  reconciliation, `static_ipv4`, and `wifi_client` canonical/rendered
  connectivity modes
- The remaining provisioning-adjacent work is AP/captive-portal UX and real
  hardware-radio validation, using the same provisioning engine boundary
