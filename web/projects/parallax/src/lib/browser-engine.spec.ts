import { create, toBinary } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";

import { BrowserEngine } from "./browser-engine";
import type { WorkerPort } from "./browser-engine";
import { ServerMessageSchema } from "./proto/parallax_pb";

/** A worker that records what the page posts and lets the test answer. */
class FakeWorker implements WorkerPort {
  public readonly posted: unknown[] = [];
  public terminated = false;
  private listener: ((event: MessageEvent<unknown>) => void) | null = null;

  public readonly postMessage = (message: unknown): void => {
    this.posted.push(message);
  };

  public readonly addEventListener = (_: "message", listener: (event: MessageEvent<unknown>) => void): void => {
    this.listener = listener;
  };

  public readonly terminate = (): void => {
    this.terminated = true;
  };

  public answer(data: unknown): void {
    this.listener?.(new MessageEvent("message", { data }));
  }

  /** The ids of the copies requested so far. */
  public copyIds(): number[] {
    return this.posted.flatMap((m) => (typeof m === "object" && m !== null && "kind" in m && m.kind === "copy" && "id" in m && typeof m.id === "number" ? [m.id] : []));
  }
}

function welcome(names: readonly string[]): Uint8Array {
  return toBinary(ServerMessageSchema, create(ServerMessageSchema, { body: { case: "welcome", value: { protocolVersion: 1, datasets: names.map((name) => ({ name })) } } }));
}

describe("BrowserEngine", () => {
  it("loads each table once and forwards protocol messages both ways", async () => {
    const worker = new FakeWorker();
    const engine = new BrowserEngine(worker);
    expect(engine.connection.location).toBe("browser");
    // The connection says hello through the worker.
    expect(worker.posted[0]).toMatchObject({ kind: "message" });
    const first = engine.copy("positions", { b: "2", a: "1" }, "/copy/positions?a=1&b=2");
    const again = engine.copy("positions", { a: "1", b: "2" }, "/copy/positions?a=1&b=2");
    expect(worker.copyIds()).toEqual([1]);
    expect(worker.posted[1]).toEqual({ kind: "copy", id: 1, dataset: "positions", parameters: { b: "2", a: "1" }, url: "/copy/positions?a=1&b=2" });
    worker.answer({ kind: "copied", id: 1, rows: 10 });
    await expect(first).resolves.toBeUndefined();
    await expect(again).resolves.toBeUndefined();
    worker.answer({ kind: "reply", bytes: welcome(["positions"]) });
    await expect(engine.connection.dataset("positions")).resolves.toMatchObject({ name: "positions" });
  });

  it("forgets a failed copy so that a later view tries again", async () => {
    const worker = new FakeWorker();
    const engine = new BrowserEngine(worker);
    const failed = engine.copy("positions", {}, "/copy/positions");
    worker.answer({ kind: "failed", id: 1, message: "the server answered 503" });
    await expect(failed).rejects.toThrow("503");
    const retried = engine.copy("positions", {}, "/copy/positions");
    expect(worker.copyIds()).toEqual([1, 2]);
    worker.answer({ kind: "copied", id: 2, rows: 1 });
    await expect(retried).resolves.toBeUndefined();
  });

  it("stops on a fatal error or a message it does not know, failing every copy and view", async () => {
    for (const bad of [
      { kind: "fatal", message: "the parallax engine did not start" },
      { kind: "reply", bytes: "not bytes" }
    ]) {
      const worker = new FakeWorker();
      const engine = new BrowserEngine(worker);
      const errors: string[] = [];
      engine.connection.open("positions", "{}", {
        frame: () => undefined,
        error: (m) => {
          errors.push(m);
        }
      });
      const pending = engine.copy("positions", {}, "/copy/positions");
      worker.answer(bad);
      await expect(pending).rejects.toThrow();
      await expect(engine.copy("trades", {}, "/copy/trades")).rejects.toThrow();
      expect(errors.length).toBe(1);
    }
  });
});
