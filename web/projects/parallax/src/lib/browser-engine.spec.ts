import { create, toBinary } from "@bufbuild/protobuf";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BrowserEngine } from "./browser-engine";
import type { WorkerPort } from "./browser-engine";
import { ServerMessageSchema } from "./proto/parallax_pb";

/** A worker that records what the page posts and lets the test answer. */
class FakeWorker implements WorkerPort {
  public readonly posted: unknown[] = [];
  public terminated = false;
  private readonly listeners = new Map<string, (event: Event) => void>();

  public readonly postMessage = (message: unknown): void => {
    this.posted.push(message);
  };

  public readonly addEventListener = (type: string, listener: (event: Event) => void): void => {
    this.listeners.set(type, listener);
  };

  public readonly terminate = (): void => {
    this.terminated = true;
  };

  public answer(data: unknown): void {
    this.listeners.get("message")?.(new MessageEvent("message", { data }));
  }

  /** Fires the event a worker fires when its script fails to load or run. */
  public crash(message: string): void {
    this.listeners.get("error")?.(new ErrorEvent("error", { message }));
  }

  /** The ids of the copies requested so far. */
  public copyIds(): number[] {
    return this.posted.flatMap((m) => (typeof m === "object" && m !== null && "kind" in m && m.kind === "copy" && "id" in m && typeof m.id === "number" ? [m.id] : []));
  }
}

function welcome(names: readonly string[]): Uint8Array {
  return toBinary(ServerMessageSchema, create(ServerMessageSchema, { body: { case: "welcome", value: { protocolVersion: 1, datasets: names.map((name) => ({ name })) } } }));
}

afterEach(() => {
  vi.useRealTimers();
});

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
      // The worker ends, releasing the copies it held.
      expect(worker.terminated).toBe(true);
    }
  });

  it("fails every copy when the worker script does not start", async () => {
    const worker = new FakeWorker();
    const engine = new BrowserEngine(worker);
    const pending = engine.copy("positions", {}, "/copy/positions");
    worker.crash("Failed to fetch a worker script");
    await expect(pending).rejects.toThrow(/did not start.*Failed to fetch a worker script/);
    expect(worker.terminated).toBe(true);
  });

  it("gives up on a copy that takes too long, and tells the worker to stop fetching it", async () => {
    vi.useFakeTimers();
    const worker = new FakeWorker();
    const engine = new BrowserEngine(worker);
    const slow = engine.copy("positions", {}, "/copy/positions", 1_000);
    const outcome = expect(slow).rejects.toThrow("longer than 1 s");
    await vi.advanceTimersByTimeAsync(1_000);
    await outcome;
    expect(worker.posted).toContainEqual({ kind: "cancel", id: 1 });
    // A late answer for the abandoned copy changes nothing, and a later view tries again.
    worker.answer({ kind: "copied", id: 1, rows: 5 });
    void engine.copy("positions", {}, "/copy/positions", 1_000).catch(() => undefined);
    expect(worker.copyIds()).toEqual([1, 2]);
  });
});
