# bunzo

An experimental agentic Linux distribution.

bunzo is an operating system built around the idea that AI agents should be a first-class citizen of the OS, not an app running on top of one. The long-term aim is a flashable image that runs on dedicated consumer-friendly devices, centered on a minimal chat-first interface, with room for proactive, policy-bounded action taken on the user's behalf.

## Status

Early prototype. The current focus is bringing up a minimal custom image on Raspberry Pi that boots into a bunzo-branded shell. See [docs/ROADMAP.md](docs/ROADMAP.md) for milestones.

## Building

bunzo is built from source with Buildroot. Three supported flows:

- **Linux (native, fastest).** Host deps from Buildroot's manual, then:
  ```
  ./scripts/bootstrap.sh            # clone buildroot (once)
  ./scripts/build.sh qemu_aarch64   # ~5–15 min on a modern box
  ./scripts/run-qemu.sh qemu_aarch64
  ```
- **macOS (Docker wrapper).** Works but slower because of Docker Desktop's VM + virtiofs. Buildroot's heavy I/O is routed onto Docker named volumes to avoid virtiofs SIGBUS:
  ```
  ./scripts/build-docker.sh qemu_aarch64
  ./scripts/run-qemu.sh qemu_aarch64
  ```
- **macOS driving a remote Linux builder (recommended for iteration).** Edit locally, let a helper script push to GitHub, pull on the remote, and build there. Boot the resulting image in QEMU over SSH. One-time setup:
  ```
  cp scripts/remote.env.example scripts/remote.env.local
  $EDITOR scripts/remote.env.local         # host/port/user/path
  ssh-copy-id -p <port> <user>@<host>      # optional but recommended
  ```
  Then:
  ```
  ./scripts/remote-build.sh qemu_aarch64   # push + remote pull + remote build
  ./scripts/remote-qemu.sh  qemu_aarch64   # boot on remote, serial over ssh
  ```
  `scripts/remote.env.local` is gitignored so host details stay off GitHub.

## Documentation

- [Vision](docs/VISION.md) — where the project is heading
- [Architecture](docs/ARCHITECTURE.md) — what exists now vs what comes later
- [Roadmap](docs/ROADMAP.md) — milestones and current focus

## License

MIT. See [LICENSE](LICENSE).
