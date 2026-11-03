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
  readonly postMessage: (message: unknown) => void;
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
  z.strictObject({ kind: z.literal("fatal"), message: z.string() })
]);

interface Pending {
  readonly resolve: () => void;
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
      this.pending.set(id, { resolve, reject });
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

  private settle(id: number, error: Error | null): void {
    const pending = this.pending.get(id);
    this.pending.delete(id);
    if (error === null) {
      pending?.resolve();
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
