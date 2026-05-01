# bunzo

An experimental agentic Linux distribution built from source with Buildroot.

bunzo treats the agent runtime as part of the OS, not an app on top of one.
The current development target is a QEMU-bootable image with a local shell,
durable runtime state, runtime policy, and proactive jobs that stay inside the
same task/policy/audit model as interactive work.

## Current state

- M1 through M9 are operationally closed in the QEMU development loop.
- Runtime, provisioning, scheduler, browser control, and approval paths all
  use the same durable task/policy/audit model.
- Hardware replay has started with the `cm5_nano_a` target. The image path
  builds `sdcard.img`; hardware smoke replay is still outstanding.

## Recommended doc load order

For a new coding thread, do not load every markdown file by default.

1. `STATE.md` first
2. `docs/ROADMAP.md` only for milestone status or open work
3. `docs/ARCHITECTURE.md` only for runtime/service boundaries
4. `docs/PROVISIONING.md` only for M7 work
5. `docs/VISION.md` only for product-direction questions

## Build

The default workflow is: edit locally, build on the remote Linux host, boot
QEMU on that same host.

### Remote Linux builder (default)

One-time setup:

```sh
cp scripts/remote.env.example scripts/remote.env.local
$EDITOR scripts/remote.env.local
ssh-copy-id -p 2299 filextract@filextract-server
```

Then:

```sh
./scripts/remote-build.sh qemu_aarch64
./scripts/remote-qemu.sh qemu_aarch64
```

Important: `remote-build.sh` only sees pushed Git state. For uncommitted local
changes, sync the tree to the remote host first or commit/push before building.

For CM5-NANO-A hardware image work:

```sh
./scripts/remote-build.sh cm5_nano_a
./scripts/remote-fetch-image.sh cm5_nano_a --dry-run
```

See [docs/HARDWARE_REPLAY.md](docs/HARDWARE_REPLAY.md) before flashing or
claiming hardware replay.

### Native Linux build

```sh
./scripts/bootstrap.sh
./scripts/build.sh qemu_aarch64
./scripts/run-qemu.sh qemu_aarch64
```

### macOS Docker fallback

Use this only when the remote builder is unavailable:

```sh
./scripts/build-docker.sh qemu_aarch64
./scripts/run-qemu.sh qemu_aarch64
```

## Runtime notes

- Canonical runtime state lives at `/var/lib/bunzo/state/runtime.sqlite3`.
- The JSONL ledger at `/var/lib/bunzo/ledger.jsonl` is an audit sink, not the
  canonical runtime store.
- `bunzo-shell` currently supports `/setup`, `/conversations`, `/tasks`,
  `/policy`, `/approve`, `/approvals`, and `/jobs`.
- The OpenAI backend is currently limited to the GPT-5.4 family, with
  `gpt-5.4-mini` as the current interactive default.

## Docs

- [STATE.md](STATE.md) — compact working state for new threads
- [docs/ROADMAP.md](docs/ROADMAP.md) — milestone status and open work
- [docs/HARDWARE_REPLAY.md](docs/HARDWARE_REPLAY.md) — CM5-NANO-A hardware
  replay runbook
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — current runtime/service shape
- [docs/PROVISIONING.md](docs/PROVISIONING.md) — M7 target design
- [docs/VISION.md](docs/VISION.md) — long-term product direction
- [docs/FOUNDATIONS.md](docs/FOUNDATIONS.md) — why the project order is state
  → policy → provisioning → scheduler

## License

MIT. See [LICENSE](LICENSE).
