---
name: bmo-cache-siren
description: Read how well a harness's prompt cache is holding, and widen or narrow the siren that sounds when new tokens jump, with the bmo-cache-siren MCP server. Use when a task asks why a session is spending so many tokens, whether a session's cache is being reused or re-prefilled, which sessions went cold, or when the cache siren should be quieted, armed again or given another threshold or cache lifetime.
---

# bmo-cache-siren

The cache-siren service watches every model request a harness makes: it tails the harness's transcripts
and takes the usage a runner posts, records each request with its cache hits (`cachedTokenCount`) and its
new tokens (`newTokenCount`), names the likely cause of a cache miss, and sounds a siren when a request or
a burst of requests sends a large context up again. The `bmo-cache-siren` MCP server reaches the service
through its own client, one tool for each route a harness needs.

## Starting the server

Register the server with the harness as a stdio MCP server:

```
bmo-cache-siren mcp
```

It reaches the service at the port the daemon's config file names (`--config`, else `config.json` in the data
directory or the install root; 9022 where none stands) with the token in the service's token file. `--address`
(`CACHE_SIREN_ADDRESS`) names another service, `--token` (`CACHE_SIREN_TOKEN`) gives the token itself, and
`--token-file` or `--data-dir` say where the file is.

## Reading the cache

- `cache_stats` sums the service's window by session: requests, new, cached and output tokens,
  `cachedShare` (cached over cached and new), cold requests and when the session was first and last seen,
  the latest session first, with `sirenCount` beside them. Give `sessionId` for one session and `since`
  (epoch milliseconds) to start the sums later; without `since` they start at the oldest event the
  window holds.
- A healthy session reads most of its context back: a high `cachedShare` and few cold requests. A cold
  request read nothing back and took many new tokens.
- `cache_events` reads the events themselves, a page at a time: without `after` the newest `limit`
  (100 by default, at most 1000), oldest first; with `after` (an event's `id`) the ones past it, and
  `more` says whether more stand. Each event is `usage.created` (one request), `siren.sounded` (a red
  request or a burst raised the siren) or `settings.changed`.
- A Usage's `note` names the cause of a cold request: `session_start`, `model_switch`, `effort_model`,
  `compact`, `ttl_expired` (the gap passed the cache lifetime), `prefix_changed` (less than half the
  previous context was read back), `tool_search`, `image` or `unknown`.

## The siren

- `cache_settings` reads `armed`, `ttlMs` (the cache lifetime the service assumes when it judges
  `ttl_expired`) and `threshold` (the new tokens that make a cold or growth request red).
- `cache_settings_update` changes any of them; a field left out stays. Quieting the siren (`armed`
  false) stops the sound and the voice, and sirens are still recorded. A `threshold` must stay above the
  service's yellow line; one at or under it is refused with `conflict`.

## Refusals

A refused call comes back as a tool error whose text is the service's `status`, `code` and `error`:
`bad-request` for an argument of the wrong shape, `conflict` for a threshold the settings do not allow,
`gone` for an `after` from before the window (read again without `after`), and `unauthorized` where the
token is not the service's.
