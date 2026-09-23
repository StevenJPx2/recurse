import { build } from "esbuild"
import { copyFileSync, existsSync, rmSync } from "node:fs"
import path from "node:path"

// Host SDKs are supplied by the host at runtime and never bundled.
const EXTERNAL = ["node:*", "@opencode/*", "@earendil-works/*", "typebox", "typebox/*"]

const ENTRIES = [
  { entry: "packages/client/src/index.ts", outdir: "packages/client/dist" },
  { entry: "adapters/opencode/src/index.ts", outdir: "adapters/opencode/dist", guidance: true },
  { entry: "adapters/pi/src/index.ts", outdir: "adapters/pi/dist", guidance: true },
]

for (const { entry, outdir, guidance } of ENTRIES) {
  rmSync(outdir, { recursive: true, force: true })
  await build({
    entryPoints: [entry],
    outfile: path.join(outdir, "index.js"),
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node22",
    external: EXTERNAL,
    conditions: ["source"],
    sourcemap: true,
    logLevel: "warning",
  })

  // Adapters read GUIDANCE.md next to their bundle so installed copies never reach into the repo.
  if (guidance && existsSync("GUIDANCE.md")) copyFileSync("GUIDANCE.md", path.join(outdir, "GUIDANCE.md"))
}

console.log(`built ${ENTRIES.length} bundles`)
