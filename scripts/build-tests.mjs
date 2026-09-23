import { build } from "esbuild"
import { readdirSync, rmSync } from "node:fs"
import path from "node:path"

const EXTERNAL = ["node:*", "@opencode/*", "@earendil-works/*", "typebox", "typebox/*"]
const ROOTS = ["packages", "adapters"]

let count = 0

for (const root of ROOTS) {
  for (const pkg of readdirSync(root, { withFileTypes: true })) {
    if (!pkg.isDirectory()) continue

    const testDir = path.join(root, pkg.name, "test")
    const outdir = path.join(root, pkg.name, "dist-tests")
    const files = listTests(testDir)

    rmSync(outdir, { recursive: true, force: true })
    if (files.length === 0) continue

    await build({
      entryPoints: files,
      outdir,
      outbase: testDir,
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node22",
      external: EXTERNAL,
    conditions: ["source"],
      sourcemap: "inline",
      logLevel: "warning",
    })
    count += files.length
  }
}

function listTests(directory) {
  try {
    return readdirSync(directory, { withFileTypes: true, recursive: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".test.ts"))
      .map((entry) => path.join(entry.parentPath, entry.name))
  } catch {
    return []
  }
}

console.log(`built ${count} test bundles`)
