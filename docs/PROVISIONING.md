# bunzo — provisioning spec

This document defines the intended first-boot and reconfiguration path for
bunzo as a product. It assumes the end goal is a flashable **agentic OS** that
can run on both headless devices (Pi-class boards, mini PCs in closets, audio
appliances) and local-I/O devices (desktop PCs, laptops, mini PCs with a
display + keyboard + mouse).

The key rule is simple:

- **One provisioning engine**
- **Multiple setup surfaces**
- **Same resulting persisted config**

`bunzo-shell` stays valid on local-I/O hardware. Phone/browser-led setup exists
so headless hardware is not hostile to normal people.

## Product goals

1. A user can flash bunzo, power it on, and get to a usable ready state
   without needing Linux knowledge.
2. Headless devices can be set up from a phone.
3. Devices with a local display and keyboard can be set up locally.
4. The AI only takes over after the deterministic prerequisites are satisfied:
   device identity, connectivity, provider auth, and a persisted config.
5. Re-entering setup later should be explicit, safe, and easy.

## Non-goals for v1

1. Full account systems or cloud dashboards.
2. Fancy graphical local desktop UIs.
3. Provider-specific OAuth/device-code login flows on day one.
4. Letting the LLM "figure out" low-level setup primitives like Wi-Fi joins or
   system clock sync. Those should be implemented by ordinary system services.

## User-visible setup surfaces

### 1. Headless path

Primary target: Pi-class boards or any device with no local screen.

Recommended v1 flow:

1. First boot enters provisioning mode.
2. Device advertises a temporary AP such as `bunzo-ABCD`.
3. Phone joins the AP.
4. Captive portal or `http://setup.bunzo.local/` opens.
5. User completes the setup wizard.
6. Device applies config, validates it, exits provisioning mode, and becomes
   ready.

Fallbacks:

1. Ethernet-connected device exposes `http://bunzo.local/setup`.
2. Factory reset / "re-enter setup mode" path always exists.

### 2. Local path

Primary target: desktop PCs, laptops, mini PCs connected to a monitor, or
developers on serial/console.

Recommended v1 flow:

1. First boot still enters provisioning mode.
2. `bunzo-shell` detects `unprovisioned` state and shows a setup-first flow.
3. The user completes the same core steps locally:
   - device name
   - connectivity
   - provider
   - credentials
4. The same persisted config is written.
5. bunzo exits provisioning mode and starts normal agent mode.

The current `/setup` shell command is the seed of this path, but long term it
should call the provisioning engine rather than writing config files directly.

## Provisioning engine

Introduce a dedicated provisioning service:

- `bunzo-provisiond`

Responsibilities:

1. Own the first-boot state machine.
2. Expose a local API for setup frontends.
3. Manage provisional networking setup.
4. Validate connectivity and provider credentials.
5. Write the persisted config.
6. Hand off cleanly into normal bunzo runtime.

Frontends:

1. `bunzo-shell` local setup frontend
2. `bunzo-setup-ui` phone/browser frontend

Both frontends should drive the same backend API and same state machine.

## State machine

Recommended provisioning states:

1. `unprovisioned`
   No valid persisted setup exists.
2. `naming`
   Collect device name.
3. `connectivity`
   Establish internet access.
4. `provider`
   Choose provider + auth method.
5. `validating`
   Test network, test credentials, render runtime config.
6. `ready`
   Device exits provisioning mode and starts normal bunzo operation.
7. `failed_recoverable`
   Validation failed; user can retry from the relevant step.

Transitions should be explicit and restart-safe. If power is lost during setup,
the device should resume from the last persisted incomplete state, not start
from scratch unless the user requests reset.

## What setup asks

Keep the wizard minimal. It should ask only:

1. **Device name**
   Example: `Kitchen bunzo`, `Office box`, `Desk agent`
2. **How should this device get online?**
   - Ethernet
   - Wi-Fi
3. **Which AI provider do you want?**
   v1:
   - OpenAI
   - OpenRouter
   later:
   - ChatGPT-style login/device-code flow
   - local model mode
4. **How do you want to authenticate?**
   v1:
   - paste API key
   later:
   - browser/device login

That is enough to make the device useful.

## Deterministic vs agentic responsibilities

### Deterministic system-owned setup

These should be implemented by ordinary code/services, not delegated to the
LLM:

1. Wi-Fi scan/join
2. Ethernet detection
3. DHCP/static network application
4. Temporary AP lifecycle
5. Captive portal behavior
6. Credential validation calls
7. Persisting secrets/config
8. Clock sync via NTP/systemd-timesyncd or equivalent

### AI-owned follow-up work

Once setup succeeds and the runtime is online, bunzo can take over softer
tasks such as:

1. Explaining what it finished configuring
2. Suggesting or confirming timezone
3. Inspecting device capabilities
4. Suggesting next steps or useful tasks
5. Personalization

The model can orchestrate or explain these actions, but the low-level setup
should still be backed by deterministic services.

## Networking design

Recommended v1 networking split:

1. **Ethernet**
   Managed by standard Linux networking (`systemd-networkd` or equivalent).
2. **Wi-Fi station mode**
   Managed by a standard Wi-Fi stack (`wpa_supplicant` or `iwd`).
3. **Provisioning AP mode**
   Temporary AP service plus DHCP/DNS for the setup network.

For headless v1, the easiest boring stack is:

1. `hostapd` for the temporary AP
2. `dnsmasq` for DHCP + captive DNS
3. ordinary HTTP server in `bunzo-setup-ui`
4. ordinary station-mode client stack for joining the user's Wi-Fi

This is not the smallest possible stack, but it is predictable.

## Wi-Fi join flow

Headless or local, the logical flow should be the same:

1. Scan visible networks.
2. Present SSIDs.
3. User chooses SSID.
4. User enters password.
5. bunzo attempts join.
6. bunzo validates:
   - association
   - DHCP lease
   - DNS resolution
   - outbound HTTPS reachability
7. If validation fails, stay in `connectivity` with a plain error message.

## Provider flow

Recommended v1 providers:

1. OpenAI API key
2. OpenRouter API key

Recommended v1 behavior:

1. User selects provider.
2. User pastes key.
3. bunzo performs a cheap live validation call.
4. If valid, persist and proceed.
5. If invalid, stay in `provider` and explain the failure plainly.

Later:

1. ChatGPT-like login flow where supported
2. Multiple saved providers
3. Per-task model routing policy UI

## Persisted config boundary

The main architectural rule: `/etc` is not the source of truth.

Provisioning should write to persisted state under a writable bunzo-owned
config root, for example:

- `/var/lib/bunzo/config/device.toml`
- `/var/lib/bunzo/config/network.toml`
- `/var/lib/bunzo/config/provider.toml`
- `/var/lib/bunzo/secrets/openai.key`
- `/var/lib/bunzo/secrets/openrouter.key`

Then a renderer or activation step materializes runtime-facing files such as:

- `/etc/bunzo/bunzod.toml`
- network service configs
- hostname files

That keeps first-boot state, reconfiguration, and future read-only-rootfs work
compatible.

## Suggested persisted schema

Example only, not a locked ABI:

```toml
# /var/lib/bunzo/config/device.toml
state_version = 1
device_name = "Kitchen bunzo"
provisioning_state = "ready"
setup_surface = "phone"
```

```toml
# /var/lib/bunzo/config/network.toml
kind = "wifi"
ssid = "Home Wi-Fi"
dhcp = true
timezone = "America/Chicago"
timezone_source = "user_confirmed"
```

```toml
# /var/lib/bunzo/config/provider.toml
kind = "openai"
model = "gpt-5.4-mini"
api_key_path = "/var/lib/bunzo/secrets/openai.key"
```

The runtime render step can translate that into:

```toml
# /etc/bunzo/bunzod.toml
[backend]
kind = "openai"
model = "gpt-5.4-mini"
api_key_path = "/var/lib/bunzo/secrets/openai.key"
```

## Local shell behavior

On devices with local I/O:

1. If state is `unprovisioned`, `bunzo-shell` should start in setup mode.
2. `/setup` should become "enter provisioning flow locally", not "write ad hoc
   files directly".
3. The local flow should ask for the same fields as the phone flow.
4. Advanced/recovery commands can still exist, but should be outside the happy
   path.

This keeps the desktop-PC experience natural while preserving one config model.

## Headless browser behavior

The headless wizard should be deliberately boring:

1. Welcome
2. Name this device
3. Get online
4. Choose provider
5. Enter credentials
6. Testing
7. Ready

No freeform AI chat until the user has passed the deterministic prerequisites.

## Handoff into normal runtime

When provisioning reaches `ready`:

1. Persist config
2. Render runtime config
3. Disable temporary AP/captive services
4. Start normal networking mode
5. Start `bunzod`
6. Start the normal shell/session mode
7. Mark provisioning complete

At that point bunzo can present:

1. a local shell if local I/O exists
2. a phone client/browser entry if remote/local-phone access exists
3. an AI-generated summary of what it configured

## Reset / re-enter setup

There must always be an explicit path to re-enter provisioning:

1. physical button / long-press when supported
2. local shell command
3. phone/browser UI action
4. factory-reset state wipe

This should not require reflashing the image.

## Recommended implementation order

1. Define persisted config files and renderer boundary.
2. Introduce `bunzo-provisiond` with the state machine only.
3. Teach `bunzo-shell` to call the provisioning API instead of writing config
   directly.
4. Add Ethernet + local setup path.
5. Add headless AP + captive portal.
6. Add Wi-Fi scan/join.
7. Add provider validation.
8. Add post-setup AI follow-up.

## Why this design

It satisfies all the product constraints at once:

1. Works on headless devices.
2. Works on desktop-class devices.
3. Keeps setup simple for non-technical users.
4. Keeps the AI in the loop where it adds value.
5. Keeps deterministic infrastructure deterministic.
6. Avoids building two incompatible onboarding systems.
