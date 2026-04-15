# bunzo — vision

bunzo is an agentic Linux distribution. It treats AI agents as a first-class part of the operating system — not as an application running on top of one.

## What we are trying to build

In its mature form, bunzo should be:

- **A flashable OS image** that runs on dedicated consumer devices (Pi-class boards, mini PCs, repurposed laptops).
- **Trivial to install.** One drag-and-drop to a flasher, one reboot, and you are in.
- **Chat-first.** The primary interface is a minimal conversational shell. Buttons are the exception, not the rule.
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

A user flashes bunzo onto a Pi, plugs it into a monitor (or pairs a phone), talks to it, and it does something useful — without the user ever opening a terminal emulator themselves.
