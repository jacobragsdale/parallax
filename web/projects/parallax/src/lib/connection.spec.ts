import { create, toBinary } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";

import { Connection } from "./connection";
import { ServerMessageSchema } from "./proto/parallax_pb";

function connection(): { readonly connection: Connection; readonly sent: Uint8Array[] } {
  const sent: Uint8Array[] = [];
  const c = new Connection({
    send: (bytes) => {
      sent.push(bytes);
    },
    close: () => undefined
  });
  return { connection: c, sent };
}

describe("Connection", () => {
  it("fails pending fetches and tells each view when the transport closes", async () => {
    const { connection: c, sent } = connection();
    const errors: string[] = [];
    const view = c.open("positions", "{}", {
      frame: () => undefined,
      error: (m) => {
        errors.push(m);
      }
    });
    const fetch = view.fetch({ firstRow: 0, rowCount: 10, firstColumn: 0, columnCount: 2 });
    expect(sent.length).toBe(3);
    c.disconnect("gone");
    await expect(fetch).rejects.toThrow("gone");
    expect(errors).toEqual(["gone"]);
    expect(view.isClosed).toBe(true);
    await expect(view.fetch({ firstRow: 0, rowCount: 1, firstColumn: 0, columnCount: 1 })).rejects.toThrow("closed");
  });

  it("fails pending fetches when a view closes", async () => {
    const { connection: c } = connection();
    const view = c.open("positions", "{}", { frame: () => undefined });
    const fetch = view.fetch({ firstRow: 0, rowCount: 10, firstColumn: 0, columnCount: 2 });
    view.close();
    await expect(fetch).rejects.toThrow("closed");
  });
});

describe("Connection.dataset", () => {
  it("waits for the welcome, and fails if the connection closes first", async () => {
    const { connection: c } = connection();
    const asked = c.dataset("positions");
    c.receive(toBinary(ServerMessageSchema, create(ServerMessageSchema, { body: { case: "welcome", value: { protocolVersion: 1, datasets: [{ name: "positions", browser: true }] } } })));
    await expect(asked).resolves.toMatchObject({ name: "positions", browser: true });
    await expect(c.dataset("trades")).resolves.toBeUndefined();
    const { connection: closed } = connection();
    const waiting = closed.dataset("positions");
    closed.disconnect("gone");
    await expect(waiting).rejects.toThrow("gone");
  });
});
