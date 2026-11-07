/**
 * The engine in the browser: a Web Worker running the WebAssembly build, holding
 * copies of server tables.
 *
 * One worker serves the page. It loads a copy of each dataset and parameter set
 * the first time a view needs it, and its connection then answers views of that
 * table with no round trip to the server.
 */

import * as z from "zod/mini";

import { Connection } from "./connection";

/** What the page needs from a worker; a `Worker` provides it. */
export interface WorkerPort {
  /** `options.transfer` lists buffers that move to the worker with the message rather than being copied. */
  readonly postMessage: (message: unknown, options?: StructuredSerializeOptions) => void;
  /** Called for `message`, `messageerror` and `error`. */
  readonly addEventListener: (type: string, listener: (event: Event) => void) => void;
  readonly terminate: () => void;
}

/** How long a copy may take to arrive and load before the view runs on the server instead. */
export const COPY_TIMEOUT_MS = 120_000;

/** Messages the worker sends; see `worker/worker.ts`. */
const outboundSchema = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("reply"), bytes: z.instanceof(Uint8Array) }),
  z.strictObject({ kind: z.literal("copied"), id: z.int(), rows: z.int().check(z.nonnegative()) }),
  z.strictObject({ kind: z.literal("failed"), id: z.int(), message: z.string() }),
  z.strictObject({ kind: z.literal("fatal"), message: z.string() }),
  z.strictObject({ kind: z.literal("loaded"), id: z.int(), rows: z.int().check(z.nonnegative()), ms: z.number(), wasmBytes: z.number() }),
  z.strictObject({ kind: z.literal("exported"), id: z.int(), bytes: z.instanceof(Uint8Array), ms: z.number(), wasmBytes: z.number() }),
  z.strictObject({
    kind: z.literal("stats"),
    id: z.int(),
    wasmBytes: z.number(),
    timeOrigin: z.number(),
    marks: z.record(z.string(), z.number()),
    resources: z.array(
      z.strictObject({
        name: z.string(),
        startTime: z.number(),
        responseStart: z.number(),
        responseEnd: z.number(),
        transferSize: z.number(),
        encodedBodySize: z.number(),
        decodedBodySize: z.number()
      })
    )
  })
]);

type Outbound = z.output<typeof outboundSchema>;

/** What a load of Arrow bytes reports. */
export interface Loaded {
  readonly rows: number;
  /** How long the engine took to decode and load the bytes. */
  readonly ms: number;
  /** The engine's memory afterwards, which never shrinks. */
  readonly wasmBytes: number;
}

/** An export's bytes. */
export interface Exported {
  readonly bytes: Uint8Array;
  readonly ms: number;
  readonly wasmBytes: number;
}

/** Spike 0.3: the engine's memory and start-up times, on the worker's clock. */
export type WorkerStats = Omit<Extract<Outbound, { kind: "stats" }>, "kind" | "id">;

/** How a load of Arrow bytes is keyed and stamped. */
export interface LoadOptions {
  /** The primary key column, for reloads that apply as diffs; none by default. */
  readonly primaryKey?: string | null;
  /** When the app received the bytes; now by default. */
  readonly nowMicros?: number;
  /** When the source was read, which the status bar shows; `nowMicros` by default. */
  readonly sourceAsOfMicros?: number;
  /** Spike 0.3: decode the stream in place rather than copying it. */
  readonly inPlace?: boolean;
}

interface Pending {
  readonly resolve: (message: Outbound | null) => void;
  readonly reject: (error: Error) => void;
}

/** The key a copy is held under: the dataset and its parameters, in a fixed order. */
function copyKey(dataset: string, parameters: Readonly<Record<string, string>>): string {
  return JSON.stringify([dataset, Object.entries(parameters).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))]);
}

/** The worker-hosted engine and its connection. */
export class BrowserEngine {
  /** The connection to the worker's engine. */
  public readonly connection: Connection;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly copies = new Map<string, Promise<void>>();
  private failure: string | null = null;

  constructor(private readonly worker: WorkerPort) {
    this.connection = new Connection(
      {
        send: (bytes) => {
          worker.postMessage({ kind: "message", bytes });
        },
        close: () => {
          worker.terminate();
        }
      },
      "browser"
    );
    worker.addEventListener("message", (event) => {
      this.receive(event instanceof MessageEvent ? event.data : null);
    });
    worker.addEventListener("messageerror", () => {
      this.fail("the parallax worker sent a message the page could not read");
    });
    // A script that is missing, blocked or broken never posts anything.
    worker.addEventListener("error", (event) => {
      const detail = event instanceof ErrorEvent && event.message !== "" ? `: ${event.message}` : "";
      this.fail(`the parallax worker did not start; check that its worker URL serves worker.js from the package's wasm folder${detail}`);
    });
  }

  /** Starts the worker script at `url`, which must sit beside the package's WebAssembly files. */
  public static start(url: string): BrowserEngine {
    return new BrowserEngine(new Worker(new URL(url, location.href), { type: "module", name: "parallax" }));
  }

  /**
   * Loads the copy of `dataset` for `parameters` from `url`, once: later calls for
   * the same table share the first load. A load that fails, or takes longer than
   * `timeoutMs`, is forgotten, so a later call tries again.
   */
  public copy(dataset: string, parameters: Readonly<Record<string, string>>, url: string, timeoutMs: number = COPY_TIMEOUT_MS): Promise<void> {
    if (this.failure !== null) {
      return Promise.reject(new Error(this.failure));
    }
    const key = copyKey(dataset, parameters);
    const known = this.copies.get(key);
    if (known !== undefined) {
      return known;
    }
    const id = this.nextId;
    this.nextId += 1;
    const loaded = new Promise<void>((resolve, reject) => {
      this.pending.set(id, {
        resolve: () => {
          resolve();
        },
        reject
      });
    });
    this.copies.set(key, loaded);
    const timer = setTimeout(() => {
      this.worker.postMessage({ kind: "cancel", id });
      this.settle(id, new Error(`the copy took longer than ${String(timeoutMs / 1000)} s`));
    }, timeoutMs);
    loaded.then(
      () => {
        clearTimeout(timer);
      },
      () => {
        clearTimeout(timer);
        this.copies.delete(key);
      }
    );
    this.worker.postMessage({ kind: "copy", id, dataset, parameters: { ...parameters }, url });
    return loaded;
  }

  /**
   * Loads Arrow IPC stream bytes that the app holds, such as its API's response, as `dataset`. The buffer moves
   * to the worker, so the page can no longer read it, and it is never copied between threads. A second load of
   * the same dataset reloads it.
   */
  public async load(dataset: string, bytes: ArrayBuffer, options: LoadOptions = {}): Promise<Loaded> {
    const nowMicros = options.nowMicros ?? Date.now() * 1000;
    const message = await this.request((id) => {
      this.worker.postMessage(
        { kind: "load", id, dataset, bytes, primaryKey: options.primaryKey ?? null, nowMicros, sourceAsOfMicros: options.sourceAsOfMicros ?? nowMicros, inPlace: options.inPlace === true },
        { transfer: [bytes] }
      );
    });
    if (message?.kind !== "loaded") {
      throw new Error("the parallax worker did not answer the load");
    }
    return { rows: message.rows, ms: message.ms, wasmBytes: message.wasmBytes };
  }

  /** Writes the view a layout describes as xlsx or CSV, in the worker, and returns its bytes. */
  public async export(layoutJson: string, format: "xlsx" | "csv", loadVersion: number | null = null): Promise<Exported> {
    const message = await this.request((id) => {
      this.worker.postMessage({ kind: "export", id, format, layout: layoutJson, loadVersion });
    });
    if (message?.kind !== "exported") {
      throw new Error("the parallax worker did not answer the export");
    }
    return { bytes: message.bytes, ms: message.ms, wasmBytes: message.wasmBytes };
  }

  /** Spike 0.3: the engine's memory and start-up times. */
  public async stats(): Promise<WorkerStats> {
    const message = await this.request((id) => {
      this.worker.postMessage({ kind: "stats", id });
    });
    if (message?.kind !== "stats") {
      throw new Error("the parallax worker did not answer for its stats");
    }
    const { kind: _kind, id: _id, ...stats } = message;
    return stats;
  }

  private request(send: (id: number) => void): Promise<Outbound | null> {
    if (this.failure !== null) {
      return Promise.reject(new Error(this.failure));
    }
    const id = this.nextId;
    this.nextId += 1;
    const answered = new Promise<Outbound | null>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    send(id);
    return answered;
  }

  private settle(id: number, error: Error | null, message: Outbound | null = null): void {
    const pending = this.pending.get(id);
    this.pending.delete(id);
    if (error === null) {
      pending?.resolve(message);
    } else {
      pending?.reject(error);
    }
  }

  private receive(data: unknown): void {
    const parsed = outboundSchema.safeParse(data);
    if (!parsed.success) {
      this.fail("the parallax worker sent a message the page does not know");
      return;
    }
    const message = parsed.data;
    switch (message.kind) {
      case "reply":
        this.connection.receive(message.bytes);
        return;
      case "copied":
        this.settle(message.id, null);
        return;
      case "failed":
        this.settle(message.id, new Error(message.message));
        return;
      case "fatal":
        this.fail(message.message);
        return;
      case "loaded":
      case "exported":
      case "stats":
        this.settle(message.id, null, message);
        return;
    }
  }

  /**
   * Stops using the worker: every pending and later copy fails, every view reports `reason`, and the worker
   * ends, releasing the copies it held.
   */
  private fail(reason: string): void {
    if (this.failure !== null) {
      return;
    }
    this.failure = reason;
    for (const id of [...this.pending.keys()]) {
      this.settle(id, new Error(reason));
    }
    this.copies.clear();
    this.connection.disconnect(reason);
    this.worker.terminate();
  }
}
