# bmo-cache-siren

A live, per-request watch of prompt-cache hits and new tokens for model requests, with a siren when
new tokens jump. It tails Claude Code transcripts without adding any logging of its own, takes
usage reported over loopback by runners that leave no transcript, classifies each request, names
the likely cause of a cache miss and sounds an alert when a request or a burst of requests
re-prefills a large context. A harness takes it as an MCP server (`bmo-cache-siren mcp`, with the
`bmo-cache-siren` skill) to read how well its own cache is holding; it calls no other service.

A cold re-prefill is easy to miss and expensive: a handful of sessions restarting cold at once can
push millions of tokens through the prefill in a minute and spend a large share of a five-hour quota
before anyone notices. The siren makes that visible as it happens.

## Install and run

The core is one Rust binary, built with Cargo, and runs on any OS.

```
cargo build --release
target/release/bmo-cache-siren serve &
xdg-open "$(target/release/bmo-cache-siren page)"      # macOS: open
target/release/bmo-cache-siren status
```

The daemon listens on `127.0.0.1:9022`. On first run it writes `config.json` with every default and a
random API token into its data directory; `docs/configuration.md` lists each key and each flag of
`serve`. The browser page is the live table, opened at the address `page` prints, which carries the
token; every other verb of the same binary talks to the running daemon through its client.

Per platform:

- **macOS:** a Swift overlay app (`macos/build.sh` builds `dist/CacheSiren.app`), `afplay` for the
  siren and launchd for autostart (`deploy/launchd/`).
- **Linux:** an always-on-top browser window stands in for the overlay, the siren plays through
  `pw-play`, `paplay` or `aplay`, whichever the host has, and systemd --user starts the daemon
  (`deploy/systemd/cache-siren.service`); `docs/porting.md` covers the rest.

## Kinds and alerts

| Kind | Meaning | Level |
| --- | --- | --- |
| `warm` | new tokens under 20K | info |
| `growth` | cache read above zero and 20K or more new tokens | yellow; red at 100K or more |
| `start` | the first request of a session | info; yellow at 20K or more new |
| `cold` | a re-prefill: little or nothing read from cache, 20K or more new | yellow; red at 100K or more |
| `refusal` | the request was refused | red |
| `quota`, `error`, `compact` | a quota stop, an API error, a compaction | yellow |
| `cmd` | a local command | info |
| `notice` | an informational line | info |
| `request` | a posted request whose usage is unknown (note `usage_unavailable`), or a posted request after a session's first that read nothing from cache and stayed under 20K new | info |
| `burst` | new tokens across all sessions within 60 seconds reach 500K | red |

A red request or burst sounds the siren, once per session per cooldown. A request served by a
local model bills nothing, so it never goes above yellow.

A cold request carries its likely cause: `session_start`, `model_switch`, `effort_model`, `compact`,
`ttl_expired` (the gap passed the cache lifetime for that mode and model), `prefix_changed` (less
than half the previous context was read back), `tool_search`, `image` or `unknown`.

The thresholds come from the per-request distribution of new tokens in a large sample of real
sessions: the median near 2K, p90 near 8K, p95 near 12K and p99 near 170K. About 3% of requests
reach 20K and under 2% reach 100K. Recalibrate them to your own distribution; they are config keys.

Runners that leave no transcript post their usage to `POST /events`; `docs/api.md` states the body,
every other route, the events, the client and the harness plugin.

## Usage

```
bmo-cache-siren                   # status
bmo-cache-siren tail 30           # the last 30 events
bmo-cache-siren stats             # cache hits and new tokens by session
bmo-cache-siren mute|unmute       # quiet or arm the siren
bmo-cache-siren settings --threshold 150000
bmo-cache-siren test              # sound the siren once
bmo-cache-siren page              # the live page's address, with its token
bmo-cache-siren mcp               # the MCP server, for a harness
bmo-cache-siren restart           # launchd on macOS, systemd --user elsewhere
bmo-cache-siren log 50
```

The verbs ask the daemon at `http://127.0.0.1:9022`, or at the address `CACHE_SIREN_ADDRESS` names,
with the token in the data directory's token file; `--json` prints the API's answer as it is.

The overlay is dragged from anywhere, resized from the bottom-right corner, and a click on a row opens
its detail; the header's `—` hides it until the next `bmo-cache-siren restart`. The `local` button
shows or leaves out rows of local models, and the browser keeps that choice.

## Operations

- The log in `~/.claude/cache-siren/cache-siren.log` has INFO, WARN and ERROR lines; access logging
  is off. Every ten minutes the daemon writes an alive line.
- If the daemon stops, the overlay retries every three seconds and the page reads its events again
  and reconnects its stream every two. Restart the service and
  check the port with `lsof -nP -iTCP:9022`.
- The event log in `~/.claude/cache-siren/events.jsonl` is pruned to `retention_days`; the newest
  events are restored into the window at start, and their numbering carries on.

## Known limits

- Transcripts are written after a response, so a row appears two to five seconds after the request.
  This is a watch, not a proxy in front of the request: it cannot stop one.
- A local model behind a proxy that reports a cache read of zero shows every request as cold.

## Subagent fan-out and the five-hour quota

The costliest pattern the siren catches is a recursive subagent fan-out during broad research. In
one measured case a single session spawned 35 subagents in eight minutes, ran 368 web searches and
spent about 27M tokens, around 30% of a five-hour quota.

What the measurements showed:

- Across several hundred sessions, normal research used 3 to 7 subagents and a few dozen searches at
  p90 to p95. The outliers used 26 to 40 subagents and well over a hundred searches: a tenfold cliff,
  not a gradual tail.
- In the costly session, accumulated search results were about 89% of the tokens, the subagents'
  first prefill about 6% and the main session's aggregation about 6%.
- 367 of the 368 searches were unique. The searches were real coverage, not waste, so capping them
  cuts coverage.

The cost comes from recursion and from many subagents prefilling at once. Limit those, not the
searches. In the shell environment, or in the `env` block of `.claude/settings.json`:

```
CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH=1      # no agent spawns agents: removes the multiplication
CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=6      # damps concurrent prefill spikes, at a little speed
CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION=20    # a safety net
```

Leave `CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION` at its default. A lean research subagent whose
tools are limited to search cuts its first prefill to about a quarter, but in deep research that
saves only about 5% overall.

## Folders

| Folder | Holds |
| --- | --- |
| `src` | the `cache_siren` library, with its client and MCP server, and the `bmo-cache-siren` binary |
| `tests` | every test, with the in-process server the route tests start and the repository-wide personal-data check |
| `package` | the TypeScript client, generated from `docs/openapi.json` |
| `skills` | the `bmo-cache-siren` skill, the harness plugin's instructions |
| `web` | the live page and the demo page the daemon serves |
| `macos` | the Swift overlay app and its build script |
| `deploy` | the systemd user unit and the launchd files |
| `docs` | the API, its OpenAPI document, the configuration and the porting guide |
| `.graph/` | the code dependency graph: one node per feature with its files and what it reads or calls, and boundary nodes for the APIs it exposes, written only by `bmo graph`. Its area files are generated in the checkout and untracked, so a fresh checkout builds them again, while `settings.toml` is the repository's configuration and stays tracked |

## Documents

- `PERSONAL-DATA.md`: the personal data no file may hold, the values it allows and the check that holds
  the rule (`tests/personal_data.rs`).
- `docs/api.md`: every HTTP route, the events, the client, the command and the harness plugin.
- `docs/openapi.json`: the same API as an OpenAPI 3.1 contract, generated from the code.
- `docs/configuration.md`: every config key, its default and when it takes effect.
- `docs/porting.md`: what is OS-independent, what is macOS only, what depends on outside services,
  and how to run on Linux.

## Credits

This project is derived from cache-siren, which designed and built the daemon, the overlay,
the classification and the fan-out measurements described here. The Rust daemon carries that design
forward. See `NOTICE`.
