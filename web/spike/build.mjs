// Spike 0.3: bundles the measurement pages into spike/dist with esbuild, beside
// the engine's worker (projects/parallax/wasm, from `pnpm build:wasm`) and the
// data generator's WebAssembly (spike/dist/datagen, from build.sh).
import { cpSync, mkdirSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const web = join(here, "..");
const pnpm = join(web, "node_modules", ".pnpm");
const esbuildDir = readdirSync(pnpm).find((d) => d.startsWith("esbuild@"));
const esbuild = createRequire(import.meta.url)(join(pnpm, esbuildDir, "node_modules", "esbuild"));
const dist = join(here, "dist");
mkdirSync(join(dist, "wasm"), { recursive: true });

const common = { bundle: true, format: "esm", target: "es2022", minify: true, legalComments: "none", logLevel: "warning" };
await esbuild.build({ ...common, entryPoints: [join(here, "harness.ts")], outfile: join(dist, "harness.js") });
await esbuild.build({ ...common, entryPoints: [join(here, "chart.ts")], outfile: join(dist, "chart.js"), metafile: true }).then((r) => {
  const out = Object.values(r.metafile.outputs)[0];
  const inputs = Object.entries(out.inputs).map(([k, v]) => [
    k.replace(/^.*node_modules\/\.pnpm\/([^/]+)\/.*$/, "$1").replace(/^\.\.\/projects\/parallax\/src\/lib\/.*$/, "parallax"),
    v.bytesInOutput
  ]);
  const byPackage = {};
  for (const [k, v] of inputs) byPackage[k.startsWith("spike/") || k.endsWith("chart.ts") ? "page" : k] = (byPackage[k] ?? 0) + v;
  console.log(JSON.stringify({ chartBundleBytes: out.bytes, byPackage }));
});
await esbuild.build({ ...common, entryPoints: [join(here, "datagen-worker.ts")], outfile: join(dist, "datagen", "datagen-worker.js"), external: ["./datagen_wasm.js"] });
for (const page of ["index.html", "chart.html"]) cpSync(join(here, page), join(dist, page));
for (const f of ["worker.js", "parallax_wasm.js", "parallax_wasm_bg.wasm"]) cpSync(join(web, "projects", "parallax", "wasm", f), join(dist, "wasm", f));
console.log(`built ${dist}`);
