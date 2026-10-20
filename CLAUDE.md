# Working in bmo-cache-siren

A daemon that watches prompt-cache hits and new tokens per model request and sounds a siren when new
tokens jump; an upstream unit that a harness takes as an MCP server, calling no other service.
`README.md` is the overview; `docs/` is canonical for the API, the configuration and porting. Read the
document for an area before changing it.

## Principles

- **It runs on its own.** The siren builds, tests and runs from this checkout alone, with no context
  from any other repository: no crate, file or setting of another repository is read.
- **Only documented APIs.** Other services and harnesses meet it only through the routes, the client
  and the MCP tools `docs/api.md` states; it calls no other service.
- **It knows no caller.** No route, field, tool or document names a caller; a poster's `scope`,
  `actorName` and `labels` are carried and never read.
- **References run one way.** The lowest service owns a shared policy value and the others read it from
  that service; this repository holds only what it uses itself.

## Layout

- `src/`: the `cache_siren` library and the `bmo-cache-siren` binary.
  - `main.rs`: the binary: `serve` runs the daemon, `mcp` the MCP server, `openapi` prints the
    contract, every other verb asks the running daemon through the client.
  - `wire.rs`: every body the API takes and answers; `openapi.rs` writes `docs/openapi.json` from them.
  - `engine.rs`: tailing, the event window, the event log, the burst window, the settings and the stats;
    `transcript.rs` reads one transcript line, `posted.rs` one posted usage, `event.rs` the level.
  - `attribution.rs` and `config.rs`: the kinds, the cause ladder, the configuration and the settings'
    keys.
  - `server.rs`: the HTTP routes, the stream and the token; `token.rs`: the token file; `clocks.rs`:
    the scan, prune and alive clocks; `siren.rs`: the cooldown, the player and the voice.
  - `client.rs`: the Rust client; `mcp.rs`: the MCP server's tools over it; `cli.rs`: the verbs' lines.
- `package/`: the TypeScript client, its types generated from `docs/openapi.json`.
- `skills/bmo-cache-siren/`: the harness plugin's skill.
- `web/`: the live page and the demo page the daemon serves.
- `macos/`: the Swift overlay app and its build script.
- `deploy/`: the systemd user unit and the launchd files, with install-time placeholders.
- `tests/`: every Rust test; `tests/siren_server/` is the in-process server the route tests start.
- `docs/`: `api.md`, `openapi.json`, `configuration.md`, `porting.md`.
- `PERSONAL-DATA.md`: the personal data no file may hold, the values it allows and the check that holds
  the rule (`tests/personal_data.rs`).
- `testbed.toml`: how the repository is checked, tested, built and deployed.

## Rules

- The daemon is built only from registry crates: no crate from a path, from git or of another `bmo-*`
  repository. `Cargo.toml` lists them and a test holds the list.
- No Korean in any file of the checkout: code, comments, strings, tests, scripts, pages, deploy files,
  documents and root files alike; a test walks the whole checkout and enforces it.
- Rust tests live in `tests/`, the package's in `package/tests/`. Check and test with:
  - `cargo check --all-targets`
  - `cargo test`
  - `bun run check` in `package/`
- `docs/openapi.json` is written by `bmo-cache-siren openapi`, and `package/openapi.d.ts` by
  `bun run generate` in `package/`; a test holds each to its source, so a change to a wire type
  writes both again.
- A change to any route, field or consumed contract in `docs/api.md` updates that document in the
  same change and raises the version in `Cargo.toml` by one revision. Until the first release the
  version is `0.1.0-rev.<n>`: `0.1.0` was the base, each later bump is one revision, and major, minor
  and patch never move. A new config key or command-line flag goes into `docs/configuration.md`.
- No personal data in any tracked file, as `PERSONAL-DATA.md` states it: no mail address, home path,
  telephone number, token or private network address; the deploy files reach the home folder through
  `%h` or the `@HOME@` placeholder. `tests/personal_data.rs` holds the rule and reports the file, the
  line and the kind, never the value.
- No test touches a live instance's port or data directory: a test that needs the server starts one
  through `tests/siren_server/`, on an ephemeral port with a temporary data directory.
