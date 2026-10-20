// The client calls a route of the contract with the token, and reads its answer as the contract's type.

import { expect, test } from "bun:test";

import { cacheSirenClient, type Settings } from "../index";

const settings: Settings = { armed: true, ttlMs: 3_600_000, threshold: 100_000 };

test("a call reaches its route with the token and reads the answer", async () => {
  const asked: Request[] = [];
  const fetch = async (input: Request) => {
    asked.push(input);
    return new Response(JSON.stringify(settings), { headers: { "Content-Type": "application/json" } });
  };
  const client = cacheSirenClient("http://siren.test", "the-token", fetch as unknown as typeof globalThis.fetch);
  const { data, error } = await client.PATCH("/settings", { body: { armed: false } });
  expect(error).toBeUndefined();
  expect(data?.threshold).toBe(100_000);
  expect(asked[0].url).toBe("http://siren.test/settings");
  expect(asked[0].method).toBe("PATCH");
  expect(asked[0].headers.get("Authorization")).toBe("Bearer the-token");
  expect(asked[0].headers.get("Accept")).toBe("application/json");
});

test("a refusal is read in the error shape", async () => {
  const fetch = async () =>
    new Response(JSON.stringify({ code: "gone", error: "the window starts at event 7" }), {
      status: 410,
      headers: { "Content-Type": "application/json" },
    });
  const client = cacheSirenClient("http://siren.test", "the-token", fetch as unknown as typeof globalThis.fetch);
  const { data, error } = await client.GET("/events", { params: { query: { after: 1 } } });
  expect(data).toBeUndefined();
  expect(error?.code).toBe("gone");
});
