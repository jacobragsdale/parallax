import { create, toBinary } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";

import { copyUrlFor, exportUrlFor, httpBaseFor, placeView } from "./client";
import type { CopyingEngine } from "./client";
import { Connection } from "./connection";
import { ServerMessageSchema } from "./proto/parallax_pb";

describe("httpBaseFor", () => {
  it.each([
    ["ws://host:8791/ws", "http://host:8791"],
    ["wss://host/ws", "https://host"],
    ["wss://host", "https://host"],
    ["wss://host/ws/", "https://host"],
    ["wss://host/app/ws", "https://host/app"],
    ["wss://host/app/", "https://host/app"]
  ])("puts the HTTP base beside %s", (socket, expected) => {
    expect(httpBaseFor(socket)).toBe(expected);
  });
});

describe("endpoints", () => {
  it.each(["https://api.host/px", "https://api.host/px/"])("sit under the base %s", (base) => {
    expect(exportUrlFor(base)).toBe("https://api.host/px/export");
    expect(copyUrlFor(base, "positions", {})).toBe("https://api.host/px/copy/positions");
  });

  it("encode the dataset and its parameters", () => {
    expect(copyUrlFor("https://host/app", "eod prices", { as_of: "2026-11-02", fund: "A&B" })).toBe("https://host/app/copy/eod%20prices?as_of=2026-11-02&fund=A%26B");
  });
});

/** A server connection whose welcome lists `datasets`. */
function server(datasets: readonly { readonly name: string; readonly browser: boolean }[]): Connection {
  const c = new Connection({ send: () => undefined, close: () => undefined });
  c.receive(toBinary(ServerMessageSchema, create(ServerMessageSchema, { body: { case: "welcome", value: { protocolVersion: 1, datasets: [...datasets] } } })));
  return c;
}

function engine(copy: CopyingEngine["copy"]): CopyingEngine {
  return { connection: new Connection({ send: () => undefined, close: () => undefined }, "browser"), copy };
}

describe("placeView", () => {
  const datasets = [
    { name: "positions", browser: true },
    { name: "trades", browser: false }
  ];

  it("runs a dataset marked for the browser there, after loading its copy", async () => {
    const copied: string[] = [];
    const browser = engine((dataset, _, url) => {
      copied.push(`${dataset} ${url}`);
      return Promise.resolve();
    });
    const placed = await placeView(server(datasets), () => browser, "positions", {}, "/copy/positions");
    expect(placed.connection.location).toBe("browser");
    expect(placed.fallback).toBeNull();
    expect(copied).toEqual(["positions /copy/positions"]);
  });

  it("keeps other datasets, and every dataset without a worker, on the server", async () => {
    const browser = engine(() => Promise.reject(new Error("not called")));
    for (const [dataset, choose] of [
      ["trades", () => browser],
      ["unknown", () => browser],
      ["positions", () => null]
    ] as const) {
      const placed = await placeView(server(datasets), choose, dataset, {}, "/copy");
      expect(placed.connection.location).toBe("server");
      expect(placed.fallback).toBeNull();
    }
  });

  it("falls back to the server, saying why, when the copy fails", async () => {
    const browser = engine(() => Promise.reject(new Error("the server answered 403")));
    const placed = await placeView(server(datasets), () => browser, "positions", {}, "/copy/positions");
    expect(placed.connection.location).toBe("server");
    expect(placed.fallback).toContain("the server answered 403");
  });
});
