/**
 * Spike 0.3: generates positions in the browser, standing in for an app's API.
 *
 * It runs the WebAssembly build of `parallax-datagen` (the `datagen` feature of
 * `parallax-wasm`), so the bytes are exactly what `parallax-datagen` writes
 * natively for the same rows and seed, as an Arrow IPC stream. The buffer moves
 * to the page, as a `fetch` response's would, and the page ends this worker so
 * that its memory does not count against the engine's.
 */

import init, { generatePositions } from "./datagen_wasm.js";

interface Request {
  readonly rows: number;
  readonly seed: number;
  readonly hash: boolean;
}

const ready = init({ module_or_path: new URL("./datagen_wasm_bg.wasm", import.meta.url) });

function hex(buffer: ArrayBuffer): string {
  return [...new Uint8Array(buffer)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

addEventListener("message", (event: MessageEvent<Request>) => {
  const { rows, seed, hash } = event.data;
  ready
    .then(async () => {
      const started = performance.now();
      const bytes = generatePositions(rows, seed);
      const ms = performance.now() - started;
      const sha256 = hash ? hex(await crypto.subtle.digest("SHA-256", bytes)) : null;
      const buffer = bytes.buffer as ArrayBuffer;
      postMessage({ bytes: buffer, ms, sha256 }, [buffer]);
    })
    .catch((error: unknown) => {
      postMessage({ error: error instanceof Error ? error.message : String(error) });
    });
});
