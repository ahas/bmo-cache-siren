# Boundary API

Every interface by which cache-siren meets anything outside its own process: the HTTP routes it
serves, and the four contracts it consumes. Other services match against this document; the code is
one realisation of it.

## Versioning

The version is the one in `pyproject.toml` (semver). Any change to a contract stated here — a route,
a query parameter, a body or response field, its type or meaning, an error code, an SSE event name —
is a major version bump, and this document changes with it in the same commit.

## Counterparts

Each contract names today's counterpart by role, not by its code.

| Role | Today | Uses |
| --- | --- | --- |
| Collection caller | bmo-job | `POST /events`, one call per request of a runner that leaves no transcript, with the twelve `Event` fields below |
| Console reader | bmo-web | `GET /stats` and `GET /events?limit=N`, read-only |
| TTL writer | no service; an operator or a deployment script | `POST /ttl`, or editing `ttl_s` and `ttl_by_mode_model_s` in the config file and restarting the service |
| Quota source | the agent-supply service on its quota port | answers the GET the siren polls at `quota_url` |
| Service registry | no service; `heartbeat_url` is empty by default | receives the heartbeat POST when a host sets `heartbeat_url` |
| Overlay | the macOS overlay app in `macos/` and any browser | `GET /`, `/demo`, `/asset/*`, `/stream`, `/ctl` |

## Served routes

The server listens on `127.0.0.1` at the configured `port` (default 9022). Every JSON answer is UTF-8
with `Cache-Control: no-store`. Any POST whose `Content-Length` exceeds 65536 bytes is answered 413
with `{"ok": false, "error": "body too large"}`. An unknown GET path is answered 404 with
`{"ok": false, "error": "not found"}`; an unknown POST path 404 with `{"ok": false}`.

### GET `/` and GET `/index.html`

The overlay page, `web/index.html`, as `text/html`. 500 with `{ok: false, error, file}` when the file
cannot be read.

### GET `/demo`

The demo page, `web/demo.html`, which draws fixed sample rows without the daemon's data.

### GET `/asset/<name>`

Only `siren_on.png` and `siren_off.png`, read from the data directory, as `image/png`. Any other name
is answered 404 with `{"ok": false, "error": "asset not allowed"}`.

Neither image ships with this repository; a host that wants them places them in the data directory.
When the named image is missing or cannot be read, the answer is 500 with `ok` false, `error` the
operating system's message, and `file` the absolute path that was tried.

### GET `/health`

| Field | Type | Meaning |
| --- | --- | --- |
| `ok` | bool | always true when the process answers |
| `service` | string | `cache-siren` |
| `uptime_s` | int | seconds since start |
| `files` | int | transcript files watched on the last scan |
| `last_event` | string or null | `ts_kst` of the newest event |
| `events` | int | rows held in the ring |
| `quota` | object | as in `/stats` |

### GET `/stats`

| Field | Type | Meaning |
| --- | --- | --- |
| `per_min` | int[10] | new tokens per minute over the last ten minutes, oldest first; local-backend rows excluded |
| `new_60s` | int | new tokens in the last 60 seconds |
| `sessions_30m` | int | distinct non-local sessions with a request in the last 30 minutes |
| `quota` | object | below |
| `uptime_s` | int | seconds since start |
| `files` | int | transcript files watched |
| `last_event` | string or null | `ts_kst` of the newest event |
| `ctl` | object | the overlay control state, as in `/ctl` |
| `cfg` | object | `yellow_new_tokens`, `red_new_tokens`, `burst_red_tokens` |

`quota` holds the slot read from the quota source (empty until the first successful poll) plus the
siren's own estimate:

| Field | Type | Meaning |
| --- | --- | --- |
| `five_h`, `seven_d` | number or null | utilisation percent of the 5-hour and 7-day windows |
| `five_h_resets`, `seven_d_resets` | number or null | reset times as the source gave them |
| `tier` | string | the slot's display name, else the active account's name |
| `account`, `provider` | string or null | the slot's account and provider |
| `accounts` | int or null | how many accounts the source holds |
| `at` | string | local `HH:MM` of the poll |
| `pct_per_mtok` | number or null | 5-hour percent per million observed Anthropic new tokens over the rolling 5 hours |
| `window_tokens` | int | Anthropic new tokens observed over the rolling 5 hours |
| `budget` | object | `used_pct`, `headroom_pct`, `segments` (up to 12 of `{project, tokens, pct}`, largest first), `rolling_5h` (true) |

The console reader reads `per_min`, `new_60s`, `sessions_30m`, `uptime_s`, `files`, `last_event`,
the three `cfg` fields and the quota fields `five_h`, `five_h_resets`, `seven_d`, `seven_d_resets`,
`tier`, `provider` and `accounts`.

### GET `/events`

| Query | Default | Meaning |
| --- | --- | --- |
| `limit` | 80 | the newest N rows to answer (integer) |
| `since` | 0 | only rows with `seq` greater than this (integer) |
| `local` | 0 | `1` includes local-backend rows, which are left out otherwise |

Answer: `events` (the rows, oldest first) and `seq` (the newest sequence number assigned). Sequence
numbers belong to one process run: they restart from the restored ring at each start.

An event row:

| Field | Type | Meaning |
| --- | --- | --- |
| `seq` | int | position in this run |
| `ts_kst` | string | host-local `HH:MM:SS` of the request (the name is part of the contract; the zone is the host's) |
| `date_kst` | string | host-local `MM-DD` |
| `epoch` | number | Unix seconds of the request |
| `project` | string | project name: the encoded transcript folder with any configured prefix removed, or the caller's `project` |
| `session` | string | first 8 characters of `sid` |
| `sid` | string | whole session id |
| `agent` | bool or string | transcript rows: whether the transcript is a subagent's; posted rows: the caller's agent name |
| `parent` | string | first 8 characters of the parent session for a subagent, else empty |
| `cwd` | string | last part of the working directory |
| `mode` | string | `claude-code`, `codex` or `openrouter` |
| `model` | string | model identifier |
| `m` | string | readable model name |
| `backend` | string | `anthropic`, `local`, `openai` or `openrouter` |
| `kind` | string | `warm`, `growth`, `start`, `cold`, `refusal`, `quota`, `compact`, `error`, `notice`, `cmd`, `request` |
| `level` | string | `info`, `yellow` or `red` |
| `note` | string | a cause (`session_start`, `model_switch`, `effort_model`, `compact`, `ttl_expired`, `prefix_changed`, `tool_search`, `image`, `unknown`), `usage_unavailable`, or the text of a notice, error, limit or command |
| `new` | int | input plus cache-write tokens |
| `input`, `cc`, `read`, `out` | int | input, cache-write, cache-read and output tokens |
| `ctx`, `prev_ctx` | int | this request's context and the previous request's |
| `gap_s` | int or null | seconds since the session's previous request |
| `cost` | number | estimated USD for Anthropic models, 0 otherwise |
| `req` | string | a short label of what prompted the request |

A burst row (`kind` `burst`, `level` `red`) is added when the new tokens of the burst window pass
`burst_red_tokens`. It carries `seq`, `ts_kst`, `date_kst`, `epoch`, `kind`, `level`, `project` (the
largest owners, comma-joined), `session` and `m` (empty), `new`, `read`, `note`, `parts` (each
`{project, new, read, req}`, largest first), `req` (empty), `backend` (`anthropic`) and `cost` (0),
and none of the other fields. Its `note` reads `<N> req in <W>s`: the N requests within the
W-second burst window.

The console reader reads `seq`, `ts_kst`, `epoch`, `project`, `session`, `sid`, `cwd`, `agent`,
`gap_s`, `mode`, `m`, `model`, `kind`, `level`, `note`, `new` and `read`.

### GET `/stream`

Server-sent events (`text/event-stream`), held open until the client leaves.

| Query | Default | Meaning |
| --- | --- | --- |
| `since` | the newest `seq` | rows after this are sent first |

Two event names: `ev`, whose data is one event row as in `/events` (local rows included), sent as
rows arrive; and `stats`, whose data is the `/stats` object, sent about every two seconds.

### GET `/ctl` and POST `/ctl`

The overlay control state: `visible` (bool), `muted` (bool), `opacity` (number), `show_local`
(bool), `page` (string path). A POST body sets any of these keys and ignores others; the answer is the
whole state. The state is kept in the data directory across restarts. `muted` silences the siren
and the voice; the alert is still logged.

### POST `/test-siren`

No body. Sounds the siren and the voice once as a red cold alert, ignoring the cooldown. Answer:
`{"ok": true, "muted": <bool>}`.

### GET `/ttl` and POST `/ttl`

The cache-lifetime policy attribution judges `ttl_expired` by.

GET answers `ttl_s` (positive integer seconds) and `ttl_by_mode_model_s` (a table of
mode → model prefix or `*` → positive integer seconds).

POST takes a body with one or both of those keys. Each key given replaces that key whole, applied
live at once and then merged into the config file the process was started with by an atomic replace;
every other key in the file is left as written. A config path that is a symbolic link is written
through: the link's final target is replaced, the temporary file is made beside that target, and the
link itself stays in place. A mode with an empty table means that provider has no uniform cache
contract, and its requests are never judged `ttl_expired`.

Success: 200 with `ok` (true), `persisted` (whether the file was written) and the two keys as now held.

Refusals, each 400 with `{"ok": false, "error": <code>}` and nothing changed:

| Code | When |
| --- | --- |
| `body_not_json` | the body is not JSON |
| `body_not_object` | the body is not a JSON object |
| `unknown_field` | a key other than the two |
| `nothing_to_set` | neither key given |
| `ttl_s_not_positive_integer` | `ttl_s` is not an integer above zero (booleans and fractions included) |
| `ttl_table_not_object` | the table is not an object |
| `ttl_table_mode_invalid` | a mode name is empty or its value is not an object |
| `ttl_table_model_invalid` | a model prefix is empty |
| `ttl_table_seconds_not_positive_integer` | a lifetime is not an integer above zero |

When the live change applied but the file could not be written: 500 with `ok` false, `error`
`config_write_failed`, `applied` true and the two keys as now held.

The config file is also read at every start, so a writer that edits these keys in the file and
restarts the service reaches the same state.

### POST `/events`

One request of a runner that leaves no transcript. The collection caller's `Event` has these twelve
fields:

| Field | Type | Rule |
| --- | --- | --- |
| `mode` | string | `codex` or `openrouter` |
| `model` | string | required, non-blank, up to 500 characters |
| `key` | string | required, non-blank, up to 500 characters |
| `agent` | string | up to 500 characters |
| `project` | string | up to 500 characters; `key` when blank |
| `cwd` | string | up to 500 characters |
| `session` | string | up to 500 characters; the session id |
| `input_tokens` | number | non-negative |
| `cache_write_input_tokens` | number | non-negative |
| `cache_read_input_tokens` | number | non-negative |
| `output_tokens` | number | non-negative |
| `usage_known` | bool | only `true` counts as known |

A missing string is empty and a missing number is 0. Success: 200 with `{"ok": true, "seq": <int>}`;
the row then appears in `/events` and `/stream`. A body breaking a rule: 400 with `ok` false and an
English `error` naming the field. A request with unknown usage is a `request` row noted
`usage_unavailable`; otherwise the kind follows the same thresholds as transcript rows, and a session
is `seen` once a row of the same mode, session and model is in the ring.

## Consumed contracts

### Quota source

`GET <quota_url>` every 30 seconds with a 5-second timeout; nothing is asked when `quota_url` is
empty, and a failure is logged and the last reading kept. The answer is a JSON object whose `usage`
holds:

- `active`: `provider`, `account`, `name`, `accounts` — the account the source supplies from now;
- `quota_slots`: an object of slots keyed by number, each with `five_hour_utilization`,
  `five_hour_resets_at`, `seven_day_utilization`, `seven_day_resets_at`, `tier`, `account`,
  `provider` and `active`;
- `real_5h_pct` and `real_week_pct`: older fallbacks used only when the chosen slot lacks the
  utilisation fields.

The slot read is, in order: the slot with `active` exactly true; the slot whose `account` equals
`active.account`; slot `"1"`; none.

### Transcript roots

Each glob in `projects_globs` (with `~` expanded) that names a directory is a root. Under each root
the siren tails `<project>/<session>.jsonl` and `<project>/<session>/subagents/<agent>.jsonl` whose
modification time is within `recent_hours`. These are the Claude Code CLI's own transcript lines,
read and never written. It reads:

- assistant lines: `timestamp`, `cwd`, `isApiErrorMessage`, and from `message` the `id` (one
  request), `model`, `content` (tool names, ToolSearch) and `usage` (`input_tokens`,
  `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens`); a `<synthetic>` model
  line is a notice, a limit or an error;
- system lines with subtype `model_refusal_fallback` (`originalModel`, `fallbackModel`,
  `apiRefusalCategory`, `scope`), `local_command` (`content`, for `/model` and `/effort`) and
  `compact_boundary`;
- summary lines, as a compact;
- user lines, for the request label, images and tool results.

Offsets are kept in the data directory, so a restart resumes where it stopped.

### Heartbeat

`POST <heartbeat_url>` every 45 seconds, JSON, 5-second timeout; nothing is sent when
`heartbeat_url` is empty, and the answer is not read. Body: `name` (`cache-siren`), `node` (the
`node` setting, else the host name), `port`, `kind` (`service`), `engine` (`monitor`), `health_url`
(the loopback `/health`), `endpoint` (the loopback `/`), `status` (`up`) and `capabilities.notes` (an
English description).

### Voice and sound

On a red alert, unless muted and outside the per-session cooldown:

- when `sound` is true, `/usr/bin/afplay` plays `siren_file` if that path exists, else the generated
  `siren.wav` in the data directory; a host without that player logs the failure and stays silent;
- when `voice` is true and `voice_cmd` is a non-empty argument list, it is run with every `{text}`
  in each argument replaced by the English alert text. Neither output is read.

## Data directory

`~/.claude/cache-siren` holds the durable event log `events.jsonl` (pruned to `retention_days`),
`state.json` (transcript offsets), `ctl.json` (overlay state), `cache-siren.log` (structured lines,
cut to its last 4 MB past 8 MB), the generated `siren.wav` and the optional overlay images. Only the
siren writes it.
