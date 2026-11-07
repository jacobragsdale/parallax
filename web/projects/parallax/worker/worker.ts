/**
 * The parallax engine in a Web Worker: the WebAssembly build of the same session
 * code the server runs, so a dataset copied into the browser answers views as the
 * server would.
 *
 * The page sends these messages. `copy` fetches a table's copy from the
 * server's `/copy/{dataset}` endpoint and loads it; the browser decodes the
 * compressed answer natively. `load` loads Arrow IPC stream bytes the page
 * already holds, such as an app's own API response, moved here with the
 * message so that they are never copied between threads. `export` writes a
 * view's xlsx or CSV and moves its bytes back. `message` carries one encoded
 * protocol message, and every reply goes back as `reply`, in order. `stats`
 * (spike 0.3) reports the engine's memory and start-up times.
 */

import init, { Worker as Engine } from "./parallax_wasm.js";

/** A copy to fetch and load. */
interface CopyRequest {
  readonly kind: "copy";
  readonly id: number;
  readonly dataset: string;
  readonly parameters: Readonly<Record<string, string>>;
  readonly url: string;
}

/** One encoded protocol message. */
interface ProtocolMessage {
  readonly kind: "message";
  readonly bytes: Uint8Array;
}

/** The page gave up waiting for a copy. */
interface CancelCopy {
  readonly kind: "cancel";
  readonly id: number;
}

/** Arrow IPC stream bytes to load as a dataset; the buffer moves to the worker. */
interface LoadRequest {
  readonly kind: "load";
  readonly id: number;
  readonly dataset: string;
  readonly bytes: ArrayBuffer;
  readonly primaryKey: string | null;
  readonly nowMicros: number;
  readonly sourceAsOfMicros: number;
  /** Spike 0.3: decode the stream without copying it. */
  readonly inPlace: boolean;
}

/** A view to export, as its layout, which names the dataset. */
interface ExportRequest {
  readonly kind: "export";
  readonly id: number;
  readonly format: "xlsx" | "csv";
  readonly layout: string;
  readonly loadVersion: number | null;
}

/** Spike 0.3: the engine's memory and when it started. */
interface StatsRequest {
  readonly kind: "stats";
  readonly id: number;
}

type Inbound = CopyRequest | ProtocolMessage | CancelCopy | LoadRequest | ExportRequest | StatsRequest;

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isTextRecord(value: unknown): value is Readonly<Record<string, string>> {
  return isRecord(value) && Object.values(value).every((v) => typeof v === "string");
}

/** Reads a message from the page, or null if it is not one this worker knows. */
function parse(data: unknown): Inbound | null {
  if (!isRecord(data)) {
    return null;
  }
  const { kind, bytes, id, dataset, parameters, url } = data;
  if (kind === "message" && bytes instanceof Uint8Array) {
    return { kind, bytes };
  }
  if (kind === "load" && typeof id === "number" && typeof dataset === "string" && bytes instanceof ArrayBuffer) {
    const { primaryKey, nowMicros, sourceAsOfMicros, inPlace } = data;
    if ((primaryKey === null || typeof primaryKey === "string") && typeof nowMicros === "number" && typeof sourceAsOfMicros === "number") {
      return { kind, id, dataset, bytes, primaryKey, nowMicros, sourceAsOfMicros, inPlace: inPlace === true };
    }
  }
  if (kind === "export" && typeof id === "number") {
    const { format, layout, loadVersion } = data;
    if ((format === "xlsx" || format === "csv") && typeof layout === "string" && (loadVersion === null || typeof loadVersion === "number")) {
      return { kind, id, format, layout, loadVersion };
    }
  }
  if (kind === "stats" && typeof id === "number") {
    return { kind, id };
  }
  if (kind === "copy" && typeof id === "number" && typeof dataset === "string" && isTextRecord(parameters) && typeof url === "string") {
    return { kind, id, dataset, parameters, url };
  }
  if (kind === "cancel" && typeof id === "number") {
    return { kind, id };
  }
  return null;
}

const ARROW_STREAM = "application/vnd.apache.arrow.stream";

performance.mark("px-worker-start");
/** The engine's linear memory, once it has started; it grows and never shrinks. */
let memory: WebAssembly.Memory | null = null;
// Compiling apart from instantiating lets spike 0.3 time each; the browser still
// compiles while the module downloads, and caches the compiled code.
const engine: Promise<Engine> = WebAssembly.compileStreaming(fetch(new URL("./parallax_wasm_bg.wasm", import.meta.url)))
  .then((module) => {
    performance.mark("px-compiled");
    return init({ module_or_path: module });
  })
  .then((exports) => {
    memory = exports.memory;
    performance.mark("px-instantiated");
    const started = new Engine();
    performance.mark("px-ready");
    return started;
  });

function memoryBytes(): number {
  return memory?.buffer.byteLength ?? 0;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Copies being fetched, by request id, so that the page can cancel them. */
const fetching = new Map<number, AbortController>();

/** Fetches a copy and loads it, returning its row count. */
async function load(request: CopyRequest, signal: AbortSignal): Promise<number> {
  const response = await fetch(request.url, { credentials: "include", signal });
  if (!response.ok) {
    const reason = (await response.text()).trim();
    throw new Error(`the server answered ${response.status.toString()}${reason === "" ? "" : `: ${reason}`}`);
  }
  const type = response.headers.get("content-type") ?? "";
  if (!type.startsWith(ARROW_STREAM)) {
    throw new Error(`the server sent ${type === "" ? "no content type" : type}, not an Arrow stream`);
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  // A copy the page gave up on is not loaded, so that it holds no memory.
  signal.throwIfAborted();
  return Number((await engine).loadCopy(request.dataset, JSON.stringify(request.parameters), bytes));
}

function copy(request: CopyRequest): void {
  const controller = new AbortController();
  fetching.set(request.id, controller);
  load(request, controller.signal).then(
    (rows) => {
      fetching.delete(request.id);
      postMessage({ kind: "copied", id: request.id, rows });
    },
    (error: unknown) => {
      fetching.delete(request.id);
      postMessage({ kind: "failed", id: request.id, message: describe(error) });
    }
  );
}

/** Loads bytes the page moved here; they are not referenced once loaded. */
function loadBytes(e: Engine, request: LoadRequest): void {
  const started = performance.now();
  try {
    const load = request.inPlace ? e.loadTableInPlace.bind(e) : e.loadTable.bind(e);
    const rows = Number(load(request.dataset, new Uint8Array(request.bytes), request.primaryKey ?? undefined, request.nowMicros, request.sourceAsOfMicros));
    postMessage({ kind: "loaded", id: request.id, rows, ms: performance.now() - started, wasmBytes: memoryBytes() });
  } catch (error: unknown) {
    postMessage({ kind: "failed", id: request.id, message: describe(error) });
  }
}

/** Writes an export and moves its bytes to the page. */
function exportView(e: Engine, request: ExportRequest): void {
  const started = performance.now();
  try {
    const bytes = request.format === "xlsx" ? e.exportXlsx(request.layout, request.loadVersion) : e.exportCsv(request.layout, request.loadVersion);
    postMessage({ kind: "exported", id: request.id, bytes, ms: performance.now() - started, wasmBytes: memoryBytes() }, [bytes.buffer]);
  } catch (error: unknown) {
    postMessage({ kind: "failed", id: request.id, message: describe(error) });
  }
}

/** Spike 0.3: the engine's memory, its start-up marks and the resources the worker fetched. */
function stats(request: StatsRequest): void {
  const marks = Object.fromEntries(performance.getEntriesByType("mark").map((m) => [m.name, m.startTime]));
  const resources = performance.getEntriesByType("resource").map((entry) => {
    const r = entry as PerformanceResourceTiming;
    return {
      name: r.name,
      startTime: r.startTime,
      responseStart: r.responseStart,
      responseEnd: r.responseEnd,
      transferSize: r.transferSize,
      encodedBodySize: r.encodedBodySize,
      decodedBodySize: r.decodedBodySize
    };
  });
  postMessage({ kind: "stats", id: request.id, wasmBytes: memoryBytes(), timeOrigin: performance.timeOrigin, marks, resources });
}

function reply(replies: readonly unknown[]): void {
  for (const bytes of replies) {
    if (bytes instanceof Uint8Array) {
      postMessage({ kind: "reply", bytes }, [bytes.buffer]);
    }
  }
}

// Replies keep the order of the messages: callbacks on one promise run in the
// order they were added.
addEventListener("message", (event: MessageEvent<unknown>) => {
  const message = parse(event.data);
  if (message === null) {
    postMessage({ kind: "fatal", message: "the parallax worker received a message it does not know" });
    return;
  }
  if (message.kind === "copy") {
    copy(message);
    return;
  }
  if (message.kind === "cancel") {
    fetching.get(message.id)?.abort();
    return;
  }
  if (message.kind === "stats") {
    stats(message);
    return;
  }
  engine.then(
    (e) => {
      switch (message.kind) {
        case "load":
          loadBytes(e, message);
          return;
        case "export":
          exportView(e, message);
          return;
        case "message":
          reply(e.handle(message.bytes));
          return;
      }
    },
    (error: unknown) => {
      postMessage({ kind: "fatal", message: `the parallax engine did not start: ${describe(error)}` });
    }
  );
});
