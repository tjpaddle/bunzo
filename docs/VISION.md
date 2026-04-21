# bunzo — vision

Load this file only for product-direction questions. It is not needed for most
coding tasks.

bunzo is a flashable, agent-native Linux distribution. The long-term product is
a device image that can be set up by normal users, used locally when local I/O
exists, and still work well when the device is headless.

## North star

A user flashes bunzo onto a common device, powers it on, completes a short
deterministic setup flow, and then gets a chat-first system that can also run
policy-bounded proactive routines on the user's behalf.

## Product constraints

- **Screen-optional:** headless setup must work from a phone/browser, but local
  setup must remain first-class on desktop-style hardware.
- **Chat-first after setup:** deterministic setup is fine; open-ended AI chat
  should start only after prerequisites are satisfied.
- **Local-first runtime:** useful with no mandatory cloud control plane.
- **Observable and reviewable:** actions are logged and inspectable.
- **Safe by default:** capability ceilings plus explicit policy, not implicit
  agent authority.

## Not the goal

- not an LLM in the kernel
- not a chatbot bolted onto a generic distro
- not a cloud product that requires a round-trip to be useful
