# bunzo

An experimental agentic Linux distribution.

bunzo is an operating system built around the idea that AI agents should be a first-class citizen of the OS, not an app running on top of one. The long-term aim is a flashable image that runs on dedicated consumer-friendly devices, centered on a minimal chat-first interface, with room for proactive, policy-bounded action taken on the user's behalf.

## Status

Early prototype. QEMU boot, `bunzod`, and the first skill all work in principle, and `bunzo-shell` can now collect an OpenAI API key in-band via `/setup`. The current product direction is **screen-optional**: on headless hardware the user should be able to provision bunzo from a phone, while on desktop-class hardware with a display and keyboard the local shell should remain a normal first-class setup and usage path. See [docs/ROADMAP.md](docs/ROADMAP.md) for milestones.
The next planned platform phase is: durable context/task state, a real policy
engine, a provisioning service behind `/setup`, and a scheduler for proactive
jobs.

## Building

bunzo is built from source with Buildroot. For this repo's active day-to-day
development loop, the expected path is: edit locally, rebuild on the remote
Linux builder, and boot QEMU on that same remote host over SSH. The macOS
Docker wrapper remains available as a fallback, but it is not the default
iteration path.

- **Linux (native, fastest).** Host deps from Buildroot's manual, then:
  ```
  ./scripts/bootstrap.sh            # clone buildroot (once)
  ./scripts/build.sh qemu_aarch64   # ~5–15 min on a modern box
  ./scripts/run-qemu.sh qemu_aarch64
  ```
- **macOS driving a remote Linux builder (default and recommended).** Edit locally, let a helper script push to GitHub, pull on the remote, and build there. Boot the resulting image in QEMU over SSH. One-time setup:
  ```
  cp scripts/remote.env.example scripts/remote.env.local
  $EDITOR scripts/remote.env.local         # host alias/port/user/path
  ssh-copy-id -p 2299 filextract@filextract-server
  ```
  Then:
  ```
  ./scripts/remote-build.sh qemu_aarch64   # push + remote pull + remote build
  ./scripts/remote-qemu.sh  qemu_aarch64   # boot on remote, serial over ssh
  ```
  `remote-build.sh` builds the branch state that has been committed and pushed to `origin`; it does not include uncommitted local edits unless you sync them to the remote separately.
  If you want the old persistent QEMU session that survives SSH drops, run:
  ```
  BUNZO_REMOTE_QEMU_PERSIST=1 ./scripts/remote-qemu.sh qemu_aarch64
  ```
  `scripts/remote.env.local` is gitignored so host details stay off GitHub.
- **macOS (Docker wrapper, fallback only).** Supported, but slower because of Docker Desktop's VM + virtiofs. Keep this for emergencies or when the remote builder is unavailable:
  ```
  ./scripts/build-docker.sh qemu_aarch64
  ./scripts/run-qemu.sh qemu_aarch64
  ```

## Documentation

- [Vision](docs/VISION.md) — where the project is heading
- [Architecture](docs/ARCHITECTURE.md) — what exists now vs what comes later
- [Roadmap](docs/ROADMAP.md) — milestones and current focus
- [Foundations Plan](docs/FOUNDATIONS.md) — next platform phase after M4
- [Provisioning](docs/PROVISIONING.md) — first-boot and reconfiguration spec

## Backend Config

The remote OpenAI backend is configured at `/etc/bunzo/bunzod.toml`. bunzo is
currently pinned to the GPT-5.4 family only:

- `gpt-5.4`
- `gpt-5.4-mini`
- `gpt-5.4-nano`

Current recommendation for the interactive shell is `gpt-5.4-mini`. The daemon
currently uses one configured model for all requests; per-task routing between
`gpt-5.4`, `gpt-5.4-mini`, and `gpt-5.4-nano` is a later followup.

On images that include the latest `bunzo-shell`, if the backend is not
configured the shell warns immediately and supports `/setup` to paste the API
key directly on-device. `/setup` writes both `/etc/bunzo/bunzod.toml` and
`/etc/bunzo/openai.key`, then retries the request.

Example:

```toml
[backend]
kind = "openai"
model = "gpt-5.4-mini"
api_key_path = "/etc/bunzo/openai.key"
```

## License

MIT. See [LICENSE](LICENSE).
