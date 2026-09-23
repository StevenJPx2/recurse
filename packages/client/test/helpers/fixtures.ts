import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

export function fixturesDir(): string {
  let directory = path.dirname(fileURLToPath(import.meta.url))

  while (!existsSync(path.join(directory, "protocol", "fixtures"))) {
    const parent = path.dirname(directory)
    if (parent === directory) throw new Error("protocol/fixtures not found above the test bundle")

    directory = parent
  }

  return path.join(directory, "protocol", "fixtures")
}

export function fixture<T = unknown>(name: string): T {
  return JSON.parse(readFileSync(path.join(fixturesDir(), `${name}.json`), "utf8")) as T
}
