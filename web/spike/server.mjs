// Spike 0.3: serves spike/dist as a static site, as an app's own server would.
//
//   node spike/server.mjs [--port 8797] [--encoding auto|br|gzip|identity] [--no-store]
//
// `auto` answers each request with brotli or gzip as its Accept-Encoding allows;
// the others force one, so that a measurement can pick the encoding. Files are
// compressed once, brotli at quality 11 and gzip at level 9, as a build would
// precompress them. Responses may be cached for an hour unless --no-store.
import { readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  return i === -1 ? fallback : args[i + 1];
};
const port = Number(option("--port", "8797"));
const encoding = option("--encoding", "auto");
const store = !args.includes("--no-store");
const root = join(dirname(fileURLToPath(import.meta.url)), "dist");

const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".wasm": "application/wasm", ".json": "application/json" };
const compressed = new Map();

function variants(file) {
  const key = `${file}:${statSync(file).mtimeMs}`;
  let found = compressed.get(key);
  if (found === undefined) {
    const raw = readFileSync(file);
    found = {
      identity: raw,
      gzip: gzipSync(raw, { level: 9 }),
      br: brotliCompressSync(raw, { params: { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_SIZE_HINT]: raw.length } }),
      etag: `"${raw.length.toString(16)}-${Math.round(statSync(file).mtimeMs).toString(16)}"`
    };
    compressed.set(key, found);
  }
  return found;
}

function choose(accept) {
  if (encoding !== "auto") {
    return encoding;
  }
  if (/\bbr\b/.test(accept)) {
    return "br";
  }
  return /\bgzip\b/.test(accept) ? "gzip" : "identity";
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", "http://localhost");
  const path = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, "");
  let file = join(root, path === "/" ? "index.html" : path);
  if (!file.startsWith(root)) {
    response.writeHead(403).end();
    return;
  }
  try {
    if (statSync(file).isDirectory()) {
      file = join(file, "index.html");
    }
  } catch {
    response.writeHead(404, { "content-type": "text/plain" }).end(`not found: ${path}`);
    return;
  }
  const type = TYPES[extname(file)] ?? "application/octet-stream";
  const body = variants(file);
  const headers = { "content-type": type, vary: "accept-encoding", etag: body.etag, "cache-control": store ? "public, max-age=3600" : "no-store" };
  if (store && request.headers["if-none-match"] === body.etag) {
    response.writeHead(304, headers).end();
    return;
  }
  const chosen = TYPES[extname(file)] === undefined ? "identity" : choose(String(request.headers["accept-encoding"] ?? ""));
  if (chosen !== "identity") {
    headers["content-encoding"] = chosen;
  }
  headers["content-length"] = body[chosen].length;
  response.writeHead(200, headers);
  if (request.method === "HEAD") {
    response.end();
    return;
  }
  response.end(body[chosen]);
});

server.listen(port, "127.0.0.1", () => {
  const address = server.address();
  console.log(JSON.stringify({ url: `http://127.0.0.1:${typeof address === "object" && address !== null ? address.port : port}/`, encoding, store }));
});
