import { describe, expect, it } from "vitest";

import { exportUrlFor } from "./client";

describe("exportUrlFor", () => {
  it.each([
    ["ws://host:8791/ws", "http://host:8791/export"],
    ["wss://host/ws", "https://host/export"],
    ["wss://host", "https://host/export"],
    ["wss://host/ws/", "https://host/export"],
    ["wss://host/app/ws", "https://host/app/export"],
    ["wss://host/app/", "https://host/app/export"]
  ])("puts the export endpoint beside %s", (socket, expected) => {
    expect(exportUrlFor(socket)).toBe(expected);
  });
});
