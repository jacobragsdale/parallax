import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { describe, expect, it, vi } from "vitest";

import { Connection } from "./connection";
import type { FrameData } from "./frame";
import { ClientMessageSchema, ServerMessageSchema } from "./proto/parallax_pb";

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

describe("ViewHandle.measure", () => {
  it("asks the engine to measure and answers with what it wrote, or null when it found nothing", async () => {
    const { connection: c, sent } = connection();
    const view = c.open("prices", "{}", { frame: () => undefined });
    const asked = view.measure(10, 20);
    const request = fromBinary(ClientMessageSchema, sent.at(-1) ?? new Uint8Array());
    expect(request.body.case === "measureChart" && [request.body.value.from, request.body.value.to]).toEqual([10, 20]);
    const reply = (found: boolean): Uint8Array =>
      toBinary(
        ServerMessageSchema,
        create(ServerMessageSchema, {
          requestId: request.requestId,
          body: {
            case: "chartMeasure",
            value: { viewId: view.id, found, fromX: "2016-01-19", toX: "2016-02-02", fromValue: "100.00", toValue: "110.00", change: "10.00", percent: "10.00%", rows: 10n }
          }
        })
      );
    c.receive(reply(true));
    await expect(asked).resolves.toMatchObject({ change: "10.00", percent: "10.00%", rows: 10 });
    const nothing = view.measure(1, 2);
    const second = fromBinary(ClientMessageSchema, sent.at(-1) ?? new Uint8Array());
    c.receive(toBinary(ServerMessageSchema, create(ServerMessageSchema, { requestId: second.requestId, body: { case: "chartMeasure", value: { viewId: view.id, found: false } } })));
    await expect(nothing).resolves.toBeNull();
    // A measurement still waiting when the view closes fails.
    const waiting = view.measure(1, 2);
    view.close();
    await expect(waiting).rejects.toThrow("closed");
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

describe("ViewHandle request timing", () => {
  const frame = (configVersion = 0n): FrameData => ({
    viewId: 1,
    configVersion,
    loadVersion: 1n,
    firstRow: 0,
    firstColumn: 0,
    rowCount: 0,
    totalRows: 0,
    totalColumns: 0,
    sourceAsOfMicros: 0n,
    pushed: false,
    columns: [],
    groups: [],
    tree: null
  });

  it("stamps each answered frame with when the grid asked, including time queued behind another request", () => {
    let now = 1_000;
    const clock = vi.spyOn(performance, "now").mockImplementation(() => now);
    const { connection: c } = connection();
    const seen: FrameData[] = [];
    const view = c.open("positions", "{}", {
      frame: (f) => {
        seen.push(f);
      }
    });
    view.setViewport({ firstRow: 0, rowCount: 50, firstColumn: 0, columnCount: 5 });
    now = 1_010;
    // Asked while the first is in flight: queued, and timed from now.
    view.setViewport({ firstRow: 100, rowCount: 50, firstColumn: 0, columnCount: 5 });
    now = 1_200;
    // Hello was request 1 and opening the view 2, so the viewports are 3 and then 4.
    view.receiveFrame(frame(), 3n);
    view.receiveFrame(frame(), 4n);
    // A frame nobody asked for, such as a pushed refresh, has no time.
    view.receiveFrame(frame(), 99n);
    expect(seen.map((f) => f.requestedAtMs)).toEqual([1_000, 1_010, undefined]);
    clock.mockRestore();
  });
});
