import assert from "node:assert/strict"
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { createServer } from "node:net"
import os from "node:os"
import path from "node:path"
import { after, afterEach, before, describe, test } from "node:test"
import { RecurseClient, RecurseError, daemonEnvironment, ensureDaemon, formatCell, formatFailure, parseModel, resetSpawnThrottle } from "../src/index.ts"
import { FakeDaemon, RpcFailure } from "./helpers/fake-daemon.ts"

const target = { kind: "cli", id: "laptop" } as const

function rejectsWith(code: string): (error: unknown) => boolean {
  return (error) => error instanceof RecurseError && error.code === code
}

describe("RecurseClient.rpc", () => {
  const daemon = new FakeDaemon()
  let client: RecurseClient

  before(async () => {
    await daemon.start()
    client = new RecurseClient({ url: daemon.url })
  })
  after(() => daemon.stop())

  test("typed daemon errors become RecurseError", async () => {
    daemon.on("kernel.execute", () => {
      throw new RpcFailure({ code: "busy", message: "a cell is already running" })
    })

    await assert.rejects(client.rpc("kernel.execute", { target, code: "1" }), rejectsWith("busy"))
    await assert.rejects(client.rpc("nope" as never, {} as never), rejectsWith("invalid_request"))
  })

  test("malformed results are rejected", async () => {
    daemon.on("kernel.status", () => ({ state: "sleeping" }))

    await assert.rejects(client.rpc("kernel.status", { target }), rejectsWith("invalid_response"))
  })

  test("sends the bearer token and maps 401", async () => {
    daemon.token = "s3cret"
    try {
      await assert.rejects(client.rpc("health", {}), rejectsWith("unauthorized"))

      const authed = new RecurseClient({ url: daemon.url, token: "s3cret" })
      assert.equal((await authed.health()).protocol, 1)
    } finally {
      daemon.token = undefined
    }
  })

  test("health refuses a daemon speaking another protocol", async () => {
    daemon.protocol = 2
    try {
      await assert.rejects(client.health(), rejectsWith("protocol_mismatch"))
    } finally {
      daemon.protocol = 1
    }
  })

  test("an unreachable daemon is `unavailable`", async () => {
    const dead = new RecurseClient({ url: `http://127.0.0.1:${await freePort()}` })

    await assert.rejects(dead.health(), rejectsWith("unavailable"))
  })
})

describe("ensureDaemon", () => {
  const scratch = mkdtempSync(path.join(os.tmpdir(), "recurse-spawn-"))

  afterEach(() => resetSpawnThrottle())
  after(() => rmSync(scratch, { recursive: true, force: true }))

  test("returns a client when the daemon is healthy", async () => {
    const daemon = await new FakeDaemon().start()
    try {
      const client = await ensureDaemon({ env: { RECURSE_DAEMON_URL: daemon.url } })
      assert.equal(client.url, daemon.url)
    } finally {
      await daemon.stop()
    }
  })

  test("never spawns when RECURSE_DAEMON_URL is set", async () => {
    const url = `http://127.0.0.1:${await freePort()}`

    await assert.rejects(ensureDaemon({ env: { RECURSE_DAEMON_URL: url, RECURSE_BIN: "/nonexistent" } }), rejectsWith("unavailable"))
  })

  test("a protocol mismatch is fatal", async () => {
    const daemon = await new FakeDaemon().start()
    daemon.protocol = 7
    try {
      await assert.rejects(ensureDaemon({ env: { RECURSE_DAEMON_URL: daemon.url } }), rejectsWith("protocol_mismatch"))
    } finally {
      await daemon.stop()
    }
  })

  test("spawns RECURSE_BIN daemon detached with an allowlisted env and waits for health", async () => {
    const port = await freePort()
    const record = path.join(scratch, "spawned.json")
    const bin = path.join(scratch, "recurse")
    writeFileSync(bin, fakeBinary(record))
    chmodSync(bin, 0o755)

    const env = { PATH: process.env.PATH ?? "", HOME: os.homedir(), RECURSE_BIN: bin, RECURSE_DAEMON_PORT: String(port), SECRET_TOKEN: "leak" }
    const client = await ensureDaemon({ env })
    const spawned = JSON.parse(readFileSync(record, "utf8")) as { argv: string[]; env: Record<string, string>; pid: number }

    try {
      assert.equal(client.url, `http://127.0.0.1:${port}`)
      assert.deepEqual(spawned.argv, ["daemon"])
      assert.equal(spawned.env.RECURSE_DAEMON_PORT, String(port))
      assert.equal(spawned.env.SECRET_TOKEN, undefined)
    } finally {
      process.kill(spawned.pid)
    }
  })

  test("daemonEnvironment keeps only the allowlist", () => {
    assert.deepEqual(daemonEnvironment({ PATH: "/bin", XDG_STATE_HOME: "/s", RECURSE_MAX_DEPTH: "2", LC_ALL: "C", AWS_SECRET: "x" }), {
      PATH: "/bin",
      XDG_STATE_HOME: "/s",
      RECURSE_MAX_DEPTH: "2",
      LC_ALL: "C",
    })
  })
})

describe("formatting helpers", () => {
  test("formatCell shows status, streams, result, error, and truncation", () => {
    const text = formatCell({
      status: "error",
      stdout: "hi\n",
      stderr: "warn\n",
      result: null,
      error: { ename: "ZeroDivisionError", evalue: "division by zero", traceback: "Traceback…" },
      execution_count: 2,
      duration_ms: 4,
      truncated: true,
    })

    assert.match(text, /^\[error\] cell 2 · 4 ms/)
    assert.match(text, /stdout:\nhi/)
    assert.match(text, /stderr:\nwarn/)
    assert.match(text, /ZeroDivisionError: division by zero\nTraceback…/)
    assert.match(text, /output truncated/)
    assert.match(formatCell({ status: "ok", stdout: "", stderr: "", result: "2", error: null, execution_count: 1, duration_ms: 1, truncated: false }), /Out: 2$/)
  })

  test("formatFailure explains busy and unavailable", () => {
    assert.match(formatFailure(new RecurseError("busy", "x")), /^\[busy\] Another cell/)
    assert.match(formatFailure(new RecurseError("unavailable", "down")), /^\[recurse unavailable\] down/)
  })

  test("parseModel splits at the first slash", () => {
    assert.deepEqual(parseModel("openrouter/anthropic/claude"), { provider: "openrouter", model: "anthropic/claude" })
    assert.equal(parseModel("bare"), undefined)
    assert.equal(parseModel(null), undefined)
  })
})

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer()

    server.once("error", reject)
    server.listen(0, "127.0.0.1", () => {
      const address = server.address()
      server.close(() => resolve(typeof address === "object" && address ? address.port : 0))
    })
  })
}

function fakeBinary(record: string): string {
  return `#!/usr/bin/env node
const http = require("node:http")
const fs = require("node:fs")
fs.writeFileSync(${JSON.stringify(record)}, JSON.stringify({ argv: process.argv.slice(2), env: process.env, pid: process.pid }))
http.createServer((req, res) => {
  let body = ""
  req.on("data", (c) => (body += c))
  req.on("end", () => {
    const { id } = JSON.parse(body)
    res.writeHead(200, { "content-type": "application/json" })
    res.end(JSON.stringify({ id, result: { ok: true, name: "recurse", version: "0.1.0", protocol: 1, pid: process.pid, started_at: 1 } }))
  })
}).listen(Number(process.env.RECURSE_DAEMON_PORT), "127.0.0.1")
`
}
