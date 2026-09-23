import assert from "node:assert/strict"
import { afterEach, beforeEach, describe, test } from "node:test"
import { RecurseClient, type RecurseEvent } from "@recurse/client"
import { FakeDaemon, RpcFailure, waitFor } from "../../../packages/client/test/helpers/fake-daemon.ts"
import { fixture } from "../../../packages/client/test/helpers/fixtures.ts"
import { createRecurseExtension } from "../src/index.ts"
import { FakePi, FakeSdk, fakeContext } from "./helpers/fake-pi.ts"

const parent = { kind: "pi-session", id: "pi_parent" } as const
const events = fixture<Record<string, RecurseEvent>>("events")

function retarget(event: RecurseEvent): RecurseEvent {
  const payload = event.payload as Record<string, unknown>
  const child = payload.child as Record<string, unknown> | undefined

  return { ...event, target: parent, payload: child ? { ...payload, child: { ...child, parent } } : payload }
}

describe("Pi extension", () => {
  let daemon: FakeDaemon
  let pi: FakePi
  let sdk: FakeSdk
  const ctx = fakeContext()

  function load(env: NodeJS.ProcessEnv = {}): void {
    createRecurseExtension({
      env,
      guidance: "GUIDE",
      connect: async () => new RecurseClient({ url: daemon.url }),
      sdk: async () => sdk.sdk(),
    })(pi.api())
  }

  beforeEach(async () => {
    daemon = await new FakeDaemon().start()
    pi = new FakePi()
    sdk = new FakeSdk()
  })

  afterEach(async () => {
    if (pi.handlers.has("session_shutdown")) await pi.emit("session_shutdown", { reason: "quit" }, ctx)
    await daemon.stop()
  })

  test("registers ipython with a typebox schema and sequential execution", () => {
    load()

    const tool = pi.tools[0]
    assert.equal(tool?.name, "ipython")
    assert.equal(tool?.executionMode, "sequential")
    assert.deepEqual(Object.keys((tool?.parameters as { properties: object }).properties), ["code", "timeout_sec"])
  })

  test("session_start applies strict mode, registers the pi-session target, and subscribes", async () => {
    load({ RECURSE_ALLOW_TOOLS: "read" })

    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.subscribeCount === 1, 3_000, "subscription")

    assert.deepEqual(pi.activeTools, ["ipython", "read"])
    assert.deepEqual(daemon.callsTo("target.register")[0]?.params, {
      target: parent,
      cwd: "/abs/project",
      host: { name: "pi", version: "0.87.1", supports_children: true },
      model: "anthropic/claude-sonnet-5",
    })
  })

  test("RECURSE_STRICT=0 leaves the active tools alone", async () => {
    load({ RECURSE_STRICT: "0" })

    await pi.emit("session_start", { reason: "startup" }, ctx)

    assert.equal(pi.activeTools, undefined)
  })

  test("ipython executes and formats, and reports busy as text", async () => {
    let calls = 0
    daemon.on("kernel.execute", () => {
      if (calls++ > 0) throw new RpcFailure({ code: "busy", message: "running" })
      return { status: "ok", stdout: "", stderr: "", result: "2", error: null, execution_count: 1, duration_ms: 2, truncated: false }
    })
    load()

    const ok = await pi.tools[0]!.execute("call_1", { code: "1 + 1" }, undefined, undefined, ctx)
    const busy = await pi.tools[0]!.execute("call_2", { code: "2" }, undefined, undefined, ctx)

    assert.match(ok.content[0]?.text ?? "", /Out: 2$/)
    assert.match(busy.content[0]?.text ?? "", /^\[busy\]/)
    assert.deepEqual(daemon.callsTo("kernel.execute")[0]?.params, { target: parent, code: "1 + 1" })
  })

  test("before_agent_start adds the guidance section once", async () => {
    daemon.on("skills.list", () => ({ skills: [{ name: "core", description: "Work in recurse.", path: "/s/core", python: null, hidden: false }] }))
    load()
    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.subscribeCount === 1, 3_000, "subscription")

    const event = { prompt: "hi", systemPrompt: "", systemPromptOptions: { sections: {} as Record<string, string> } }
    await pi.emit("before_agent_start", event, ctx)
    event.systemPromptOptions.sections["recurse-guidance"] += "\nmarker"
    await pi.emit("before_agent_start", event, ctx)

    assert.match(event.systemPromptOptions.sections["recurse-guidance"] ?? "", /^GUIDE\n\nSkills[^]*- core: Work in recurse\.[^]*\nmarker$/)
  })

  test("events become custom messages that trigger a turn only when actionable", async () => {
    daemon.push(retarget(events["agent.message"]!))
    daemon.push(retarget(events["kernel.exited"]!))
    load()

    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    assert.deepEqual(pi.messages.map(({ message, options }) => [message.customType, message.display, options.triggerTurn, options.deliverAs]), [
      ["recurse", true, true, "steer"],
      ["recurse", true, false, "steer"],
    ])
    assert.equal(pi.messages[0]?.message.content, events["agent.message"]!.text)
    assert.equal((pi.messages[0]?.message.details as RecurseEvent).id, events["agent.message"]!.id)
  })

  test("child.spawn creates an in-process session, binds it, prompts it, and disposes it on shutdown", async () => {
    const spawn = retarget(events["child.spawn"]!)
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [pending] }))
    daemon.on("children.bind", (params) => ({ child: { ...pending, status: "running", session: params.session } }))
    daemon.push(spawn)
    load()

    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    assert.deepEqual(sdk.managers[0], ["/abs/project", undefined, { parentSession: "/sessions/pi_parent.jsonl" }])
    assert.deepEqual(sdk.created[0], {
      cwd: "/abs/project",
      sessionManager: { persisted: true },
      model: { provider: "anthropic", id: "claude-sonnet-5", resolved: true },
      sessionStartEvent: { type: "session_start", reason: "new" },
    })
    assert.deepEqual(daemon.callsTo("children.bind")[0]?.params, {
      target: parent,
      child_id: "c_7f3a9b2c",
      session: { kind: "pi-session", id: "pi_child_1" },
    })
    await waitFor(() => sdk.prompts.length === 1, 3_000, "child prompt")
    assert.equal(sdk.prompts[0], (spawn.payload as { prompt: string }).prompt)

    await pi.emit("session_shutdown", { reason: "quit" }, ctx)
    assert.equal(sdk.disposed, 1)
  })

  test("children inline exactly one recurse extension, even when recurse is also installed", async () => {
    const spawn = retarget(events["child.spawn"]!)
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [pending] }))
    daemon.on("children.bind", (params) => ({ child: { ...pending, status: "running", session: params.session } }))
    daemon.push(spawn)
    sdk.loaders = []
    load()

    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    const options = sdk.loaders[0]!
    const created = sdk.created[0] as { resourceLoader?: { reloaded: boolean } }
    assert.equal(options.cwd, "/abs/project")
    assert.equal(options.agentDir, "/agent")
    assert.equal(options.extensionFactories[0].name, "recurse")
    assert.equal(typeof options.extensionFactories[0].factory, "function")
    assert.equal(created.resourceLoader?.reloaded, true)

    const extension = (path: string, tools: string[]) => ({ path, tools: new Map(tools.map((name) => [name, {}])) })
    const installed = extension("/home/.pi/agent/extensions/recurse/index.js", ["ipython"])
    const other = extension("/home/.pi/agent/extensions/other.ts", ["todo"])
    const inline = extension("<inline:recurse>", ["ipython"])
    const result = options.extensionsOverride({ extensions: [installed, other, inline], errors: [], runtime: {} })
    assert.deepEqual(result.extensions, [other, inline])
  })

  test("a failed child session reports children.fail", async () => {
    const spawn = retarget(events["child.spawn"]!)
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [pending] }))
    daemon.on("children.fail", () => ({ child: { ...pending, status: "failed" } }))
    sdk.createError = new Error("no credentials")
    daemon.push(spawn)
    load()

    await pi.emit("session_start", { reason: "startup" }, ctx)
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    assert.deepEqual(daemon.callsTo("children.fail")[0]?.params, { target: parent, child_id: "c_7f3a9b2c", reason: "no credentials" })
    assert.equal(daemon.callsTo("children.bind").length, 0)
  })
})
