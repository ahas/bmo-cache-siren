// The cache-siren service's contract in TypeScript (docs/api.md, "The client"): the types openapi-typescript
// generates from docs/openapi.json, and a client over them that carries the token on every call.

import createClient, { type Client } from "openapi-fetch";

import type { components, paths } from "./openapi";

export type { components, paths };

type Schemas = components["schemas"];

export type Health = Schemas["Health"];
export type Event = Schemas["Event"];
export type EventPage = Schemas["EventPage"];
export type Usage = Schemas["Usage"];
export type NewUsage = Schemas["NewUsage"];
export type Siren = Schemas["Siren"];
export type Stats = Schemas["Stats"];
export type SessionStats = Schemas["SessionStats"];
export type Settings = Schemas["Settings"];
export type SettingsPatch = Schemas["SettingsPatch"];
export type SirenTest = Schemas["SirenTest"];
export type ErrorBody = Schemas["WireError"];

/**
 * A client of the cache-siren service at `baseUrl`, sending `token` as its bearer token and asking for JSON,
 * so `GET /events` answers a page. Each call is a route of the OpenAPI document, typed by it:
 * `client.GET("/stats", { params: { query: { sessionId } } })`.
 */
export function cacheSirenClient(baseUrl: string, token: string, fetch?: typeof globalThis.fetch): Client<paths> {
  return createClient<paths>({
    baseUrl,
    headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
    ...(fetch ? { fetch } : {}),
  });
}
