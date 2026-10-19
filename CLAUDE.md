# Working in bmo-cache-siren

A daemon that watches prompt-cache hits and new tokens per model request and sounds a siren when new
tokens jump. `README.md` is the overview; `docs/` is canonical for the API, the configuration and
porting. Read the document for an area before changing it.

## Principles

- **It runs on its own.** The siren builds, tests and runs from this checkout alone, with no context
  from any other repository: no crate, file or setting of another repository is read.
- **Only documented APIs.** It meets other services only through the contracts `docs/api.md` states,
  and other services meet it only through the routes stated there.
- **References run one way.** The lowest service owns a shared policy value and the others read it from
  that service; this repository holds only what it uses itself.

## Layout

- `src/`: the `cache_siren` library and the `cache-siren` binary.
  - `main.rs`: the binary: `serve` runs the daemon, every other verb asks the running daemon.
  - `engine.rs`: tailing, the ring, the event log, the burst window, the rolling buckets and the stats;
    `transcript.rs` reads one transcript line, `posted.rs` one posted request, `event.rs` the row.
  - `attribution.rs` and `config.rs`: the kinds, the cause ladder, the configuration and the TTL keys.
  - `server.rs`: the HTTP routes and the stream; `clocks.rs`: the scan, prune, quota and heartbeat
    clocks; `siren.rs`: the cooldown, the sound and the voice; `cli.rs`: the command-line verbs.
- `web/`: the live page and the demo page the daemon serves.
- `macos/`: the Swift overlay app and its build script.
- `deploy/`: the systemd user unit and the launchd files, with install-time placeholders.
- `tests/`: every test; `tests/siren_server/` is the in-process server the route tests start.
- `docs/`: `api.md`, `configuration.md`, `porting.md`.
- `testbed.toml`: how the repository is checked, tested, built and deployed.

## Rules

- The daemon is built only from registry crates: no crate from a path, from git or of another `bmo-*`
  repository. `Cargo.toml` lists them and a test holds the list.
- No Korean in any file of the checkout: code, comments, strings, tests, scripts, pages, deploy files,
  documents and root files alike; a test walks the whole checkout and enforces it.
- Tests live in `tests/`. Check and test with:
  - `cargo check --all-targets`
  - `cargo test`
- A change to any route, field or consumed contract in `docs/api.md` updates that document in the
  same change and bumps the major version in `Cargo.toml`. A new config key or command-line flag goes
  into `docs/configuration.md`.
- No personal paths, user names, user ids or host names in any file; the deploy files reach the
  home folder through `%h` or the `@HOME@` placeholder.
- No test touches a live instance's port or data directory: a test that needs the server starts one
  through `tests/siren_server/`, on an ephemeral port with a temporary data directory.
