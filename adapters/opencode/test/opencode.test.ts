import assert from "node:assert/strict"
import { afterEach, beforeEach, describe, test } from "node:test"
import { RecurseClient, RecurseError, type RecurseEvent } from "@recurse/client"
import { FakeDaemon, RpcFailure, makeEvent, waitFor } from "../../../packages/client/test/helpers/fake-daemon.ts"
import { fixture } from "../../../packages/client/test/helpers/fixtures.ts"
import { setupRecurse } from "../src/index.ts"
import { FakeHost, contextEvent, toolContext } from "./helpers/fake-host.ts"

const parent = { kind: "opencode-session", id: "ses_abc" } as const
const events = fixture<Record<string, RecurseEvent>>("events")

describe("OpenCode adapter", () => {
  let daemon: FakeDaemon
  let host: FakeHost
  let cleanup: (() => Promise<void>) | undefined

  async function setup(env: NodeJS.ProcessEnv = {}, connect?: () => Promise<RecurseClient>): Promise<void> {
    cleanup = await setupRecurse(host.context(), {
      env,
      guidance: "GUIDE",
      connect: connect ?? (async () => new RecurseClient({ url: daemon.url })),
    })
  }

  beforeEach(async () => {
    daemon = await new FakeDaemon().start()
    host = new FakeHost()
  })

  afterEach(async () => {
    await cleanup?.()
    cleanup = undefined
    await daemon.stop()
  })

  test("ipython registers the session target, then executes and formats the cell", async () => {
    daemon.on("kernel.execute", () => ({ status: "ok", stdout: "hi\n", stderr: "", result: "2", error: null, execution_count: 1, duration_ms: 3, truncated: false }))
    await setup()

    const output = await host.tool("ipython").execute({ code: "print('hi')\n1 + 1", timeout_sec: 30 }, toolContext("ses_abc"))

    assert.match(output.content, /^\[ok\] cell 1 · 3 ms\n\nstdout:\nhi\n\nOut: 2$/)
    assert.equal(host.tool("ipython").options?.codemode, false, "a direct tool survives strict pruning of Code Mode")
    assert.deepEqual(daemon.callsTo("target.register")[0]?.params, {
      target: parent,
      cwd: "/abs/project",
      host: { name: "opencode", version: "2.0.15", supports_children: true },
      model: "anthropic/claude-sonnet-5",
    })
    assert.deepEqual(daemon.callsTo("kernel.execute")[0]?.params, { target: parent, code: "print('hi')\n1 + 1", timeout_sec: 30 })

    const registerIndex = daemon.calls.findIndex((call) => call.method === "target.register")
    const executeIndex = daemon.calls.findIndex((call) => call.method === "kernel.execute")
    assert.ok(registerIndex < executeIndex)
  })

  test("busy and unavailable daemons become tool text, not exceptions", async () => {
    daemon.on("kernel.execute", () => {
      throw new RpcFailure({ code: "busy", message: "a cell is already running" })
    })
    await setup()

    assert.match((await host.tool("ipython").execute({ code: "1" }, toolContext("ses_abc"))).content, /^\[busy\]/)
    assert.match((await host.tool("ipython").execute({}, toolContext("ses_abc"))).content, /^\[invalid input\]/)

    await cleanup?.()
    host = new FakeHost()
    await setup({}, async () => {
      throw new RecurseError("unavailable", "connection refused")
    })

    assert.match((await host.tool("ipython").execute({ code: "1" }, toolContext("ses_abc"))).content, /^\[recurse unavailable\]/)
  })

  test("strict mode keeps only ipython and RECURSE_ALLOW_TOOLS", async () => {
    await setup({ RECURSE_ALLOW_TOOLS: "webfetch, todo" })
    const event = contextEvent("ses_abc", ["ipython", "read", "bash", "webfetch", "todo", "edit"])

    await host.hook("context")(event)

    assert.deepEqual(Object.keys(event.tools).sort(), ["ipython", "todo", "webfetch"])
  })

  test("RECURSE_STRICT=0 leaves the host's tools alone", async () => {
    await setup({ RECURSE_STRICT: "0" })
    const event = contextEvent("ses_abc", ["ipython", "read", "bash"])

    await host.hook("context")(event)

    assert.deepEqual(Object.keys(event.tools), ["ipython", "read", "bash"])
  })

  test("guidance and the skill list are injected once", async () => {
    daemon.on("skills.list", () => ({
      skills: [
        { name: "core", description: "How to work in recurse.", path: "/s/core/SKILL.md", python: null, hidden: false },
        { name: "recurse", description: "stub", path: "/s/recurse/SKILL.md", python: null, hidden: true },
      ],
    }))
    daemon.on("kernel.execute", () => ({ status: "ok", stdout: "", stderr: "", result: null, error: null, execution_count: 1, duration_ms: 1, truncated: false }))
    await setup()
    await host.tool("ipython").execute({ code: "1" }, toolContext("ses_abc"))

    const event = contextEvent("ses_abc", ["ipython"], [{ type: "text", text: "host system" }])
    await host.hook("context")(event)
    await host.hook("context")(event)

    const injected = event.system.filter((part) => part.text.includes("<recurse-guidance>"))
    assert.equal(injected.length, 1)
    assert.match(injected[0]?.text ?? "", /GUIDE/)
    assert.match(injected[0]?.text ?? "", /- core: How to work in recurse\./)
    assert.doesNotMatch(injected[0]?.text ?? "", /stub/)
  })

  test("the prompt hook registers and subscribes the session", async () => {
    await setup()

    await host.hook("prompt")({ sessionID: "ses_abc", prompt: { text: "hi" } })

    await waitFor(() => daemon.subscribeCount >= 1, 3_000, "subscription")
    assert.equal(daemon.callsTo("target.register").length, 1)
    await waitFor(() => Array.isArray(host.storage.get("sessions")), 3_000, "persisted sessions")
    assert.deepEqual(host.storage.get("sessions"), ["ses_abc"])
  })

  test("previously tracked sessions are subscribed again on load", async () => {
    host.storage.set("sessions", ["ses_abc"])
    await setup()

    await waitFor(() => daemon.subscribeCount >= 1, 3_000, "restored subscription")
  })

  test("child.spawn creates a native session, binds it, subscribes it, prompts it, and acks", async () => {
    const spawn = events["child.spawn"]!
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [pending] }))
    daemon.on("children.bind", (params) => ({ child: { ...pending, status: "running", session: params.session } }))
    daemon.push(spawn)
    await setup()

    await host.hook("prompt")({ sessionID: "ses_abc", prompt: { text: "go" } })
    await waitFor(() => daemon.queued(parent).length === 0, 5_000, "child.spawn acked")

    assert.deepEqual(host.creates[0], {
      title: "auth-reviewer · recurse",
      agent: "build",
      model: { providerID: "anthropic", id: "claude-sonnet-5" },
      location: { directory: "/abs/project" },
      metadata: { "recurse.parent": "ses_abc", "recurse.child_id": "c_7f3a9b2c", "recurse.name": "auth-reviewer" },
    })
    assert.deepEqual(daemon.callsTo("children.bind")[0]?.params, {
      target: parent,
      child_id: "c_7f3a9b2c",
      session: { kind: "opencode-session", id: "ses_child_1" },
    })
    assert.ok(daemon.callsTo("target.register").some((call) => (call.params.target as { id: string }).id === "ses_child_1"))
    assert.deepEqual(host.prompts, [{ sessionID: "ses_child_1", text: (spawn.payload as { prompt: string }).prompt }])
    assert.equal(host.synthetics.length, 0)
  })

  test("a failed session create reports children.fail and acks", async () => {
    const spawn = events["child.spawn"]!
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [pending] }))
    daemon.on("children.fail", () => ({ child: { ...pending, status: "failed" } }))
    host.createError = new Error("model unavailable")
    daemon.push(spawn)
    await setup()

    await host.hook("prompt")({ sessionID: "ses_abc", prompt: { text: "go" } })
    await waitFor(() => daemon.queued(parent).length === 0, 5_000, "acked after fail")

    assert.deepEqual(daemon.callsTo("children.fail")[0]?.params, { target: parent, child_id: "c_7f3a9b2c", reason: "model unavailable" })
    assert.equal(daemon.callsTo("children.bind").length, 0)
    assert.equal(host.prompts.length, 0)
  })

  test("a redelivered child.spawn for an already-bound child creates nothing", async () => {
    const spawn = events["child.spawn"]!
    const pending = (spawn.payload as { child: Record<string, unknown> }).child
    daemon.on("children.list", () => ({ children: [{ ...pending, status: "running", session: { kind: "opencode-session", id: "ses_old" } }] }))
    daemon.push(spawn)
    await setup()

    await host.hook("prompt")({ sessionID: "ses_abc", prompt: { text: "go" } })
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    assert.equal(host.creates.length, 0)
    assert.equal(host.prompts.length, 0)
  })

  test("tracked sessions are bounded; the least recently used is closed", async () => {
    cleanup = await setupRecurse(host.context(), {
      env: {},
      guidance: "GUIDE",
      maxSessions: 2,
      connect: async () => new RecurseClient({ url: daemon.url }),
    })

    for (const id of ["ses_1", "ses_2", "ses_3"]) {
      await host.hook("prompt")({ sessionID: id, prompt: { text: "hi" } })
      await waitFor(() => daemon.streams.size >= 1 && [...daemon.streams].some((stream) => stream.key.endsWith(id)), 3_000, id)
    }

    await waitFor(() => daemon.streams.size === 2, 3_000, "eviction")
    assert.deepEqual([...daemon.streams].map((stream) => stream.key).sort(), ["opencode-session:ses_2", "opencode-session:ses_3"])
  })

  test("actionable events prompt, others become synthetic context, each once", async () => {
    await setup()
    await host.hook("prompt")({ sessionID: "ses_abc", prompt: { text: "go" } })
    await waitFor(() => daemon.subscribeCount >= 1, 3_000, "subscription")

    daemon.push(events["agent.message"]!)
    daemon.push(events["kernel.exited"]!)
    daemon.push(makeEvent(parent, "agent.message:c_7f3a9b2c:1"))
    await waitFor(() => daemon.queued(parent).length === 0, 3_000, "acked")

    assert.deepEqual(host.prompts, [{ sessionID: "ses_abc", text: events["agent.message"]!.text }])
    assert.deepEqual(host.synthetics, [{ sessionID: "ses_abc", text: events["kernel.exited"]!.text }])
  })
})
