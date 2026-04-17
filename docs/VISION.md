# bunzo — vision

bunzo is an agentic Linux distribution. It treats AI agents as a first-class part of the operating system — not as an application running on top of one.

## What we are trying to build

In its mature form, bunzo should be:

- **A flashable OS image** that runs on dedicated consumer devices (Pi-class boards, mini PCs, repurposed laptops).
- **Trivial to install.** One drag-and-drop to a flasher, one reboot, and you are in.
- **Screen-optional.** The target device should not need a monitor, keyboard, or terminal emulator for normal setup, but if a display and input devices are present bunzo should use them well instead of forcing a phone-only flow.
- **Easy to provision from a phone.** On first boot, the user should be able to name the device, get it online, and choose an AI provider from a simple phone-led setup flow.
- **Chat-first after setup.** The primary long-term interface is conversational, but deterministic setup screens are allowed where they make onboarding radically simpler.
- **Local when local makes sense.** On desktop and laptop-style hardware, a normal local shell should remain a valid first-class way to set up and use bunzo.
- **Accessible remotely.** Users can reach their bunzo from a phone without a cloud detour.
- **Proactive.** Within clear policy boundaries, bunzo acts on the user's behalf — reminders, fetches, watches, routines — without being asked every time.
- **Observable.** Every action an agent takes is logged and reviewable.
- **Safe by default.** Capability-based permissions and explicit grants for skills — no "`rm -rf` your home directory because the LLM hallucinated".

## What bunzo is not

- **Not a kernel project.** The Linux kernel stays upstream. We are not putting an LLM in ring 0.
- **Not a chatbot on top of a generic distro.** The agent layer is deeply integrated with init, services, shell, and policy — but it lives in user space.
- **Not a cloud product.** bunzo should be useful with zero cloud, and optional cloud.
- **Not a replacement for general-purpose Linux.** A power user can drop to a full shell, but that is not the main interface.

## North star

A user flashes bunzo onto a widely available device and powers it on. If the device is headless, they complete setup from a phone. If the device has a local screen and input, they can complete setup right there in a normal bunzo shell or local setup surface. In both cases bunzo asks for only the essentials, gets online, connects the chosen AI provider, and then finishes the rest.
