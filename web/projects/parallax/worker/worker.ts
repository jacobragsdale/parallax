/**
 * The parallax engine in a Web Worker: the WebAssembly build of the same session
 * code the server runs, so a dataset copied into the browser answers views as the
 * server would.
 *
 * The page sends two kinds of message. `copy` fetches a table's copy from the
 * server's `/copy/{dataset}` endpoint and loads it; the browser decodes the
 * compressed answer natively. `message` carries one encoded protocol message,
 * and every reply goes back as `reply`, in order.
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

type Inbound = CopyRequest | ProtocolMessage | CancelCopy;

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
  if (kind === "copy" && typeof id === "number" && typeof dataset === "string" && isTextRecord(parameters) && typeof url === "string") {
    return { kind, id, dataset, parameters, url };
  }
  if (kind === "cancel" && typeof id === "number") {
    return { kind, id };
  }
  return null;
}

const ARROW_STREAM = "application/vnd.apache.arrow.stream";

const engine: Promise<Engine> = init({ module_or_path: new URL("./parallax_wasm_bg.wasm", import.meta.url) }).then(() => new Engine());

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
  engine.then(
    (e) => {
      reply(e.handle(message.bytes));
    },
    (error: unknown) => {
      postMessage({ kind: "fatal", message: `the parallax engine did not start: ${describe(error)}` });
    }
  );
});
