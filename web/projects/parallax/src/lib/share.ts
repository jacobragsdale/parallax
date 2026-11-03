/**
 * Layouts in links. A layout is compressed with deflate and written as base64url,
 * so a shared view fits in a URL parameter, and it is checked against the schema
 * when it is read back: a link someone edited by hand fails loudly instead of
 * opening a different view.
 */

import { parseLayout } from "./layout";
import type { Layout, LayoutInput } from "./layout";

const PREFIX = "1.";

function toBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function fromBase64Url(text: string): Uint8Array {
  const binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

async function pipe(bytes: Uint8Array, stream: CompressionStream | DecompressionStream): Promise<Uint8Array> {
  const input = new ReadableStream<BufferSource>({
    start: (controller) => {
      controller.enqueue(new Uint8Array(bytes));
      controller.close();
    }
  });
  const reader = input.pipeThrough(stream).getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  for (let next = await reader.read(); !next.done; next = await reader.read()) {
    chunks.push(next.value);
    length += next.value.length;
  }
  const out = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

/** Encodes a layout for a URL parameter. Deflate shrinks the many default fields to a few bytes each. */
export async function encodeLayout(layout: LayoutInput): Promise<string> {
  const json = JSON.stringify(parseLayout(layout));
  return PREFIX + toBase64Url(await pipe(new TextEncoder().encode(json), new CompressionStream("deflate-raw")));
}

/** Decodes and checks a layout from a URL parameter. */
export async function decodeLayout(text: string): Promise<Layout> {
  if (!text.startsWith(PREFIX)) {
    throw new Error("not a parallax layout link");
  }
  const bytes = await pipe(fromBase64Url(text.slice(PREFIX.length)), new DecompressionStream("deflate-raw"));
  const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
  return parseLayout(value);
}
