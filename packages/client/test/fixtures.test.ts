import assert from "node:assert/strict"
import { after, before, describe, test } from "node:test"
import {
  EVENT_KINDS,
  RecurseClient,
  RecurseError,
  isAgentMessageEvent,
  isBashFinishedEvent,
  isChildSpawnEvent,
  isEvent,
  isFromKernelMessage,
  isHostInfo,
  isKernelExitedEvent,
  isSseFrame,
  isTarget,
  isToKernelMessage,
  isWireError,
  type Guard,
  type RecurseEvent,
  type RpcMethod,
  type SseFrame,
} from "../src/index.ts"
import { FakeDaemon, RpcFailure } from "./helpers/fake-daemon.ts"
import { fixture } from "./helpers/fixtures.ts"

type RpcFixture = { request: { id: number; method: string; params: Record<string, unknown> }; response: { id: number; result?: unknown; error?: unknown } }

const events = fixture<Record<string, unknown>>("events")
const frames = fixture<unknown[]>("sse-frames")
const rpc = fixture<Record<string, RpcFixture>>("rpc")
const kernel = fixture<{ to_kernel: unknown[]; from_kernel: unknown[] }>("kernel")

const EVENT_GUARDS: Record<string, Guard<RecurseEvent>> = {
  "child.spawn": isChildSpawnEvent,
  "agent.message": isAgentMessageEvent,
  "bash.finished": isBashFinishedEvent,
  "kernel.exited": isKernelExitedEvent,
}

const METHODS: readonly RpcMethod[] = [
  "health",
  "target.register",
  "kernel.execute",
  "kernel.interrupt",
  "kernel.restart",
  "kernel.status",
  "children.list",
  "children.bind",
  "children.fail",
  "children.delete",
  "events.ack",
  "events.list",
  "skills.list",
]

describe("protocol fixtures", () => {
  test("every event fixture matches its kind's guard and round-trips", () => {
    const kinds = new Set<string>()

    for (const [name, value] of Object.entries(events)) {
      assert.ok(isEvent(value), `${name} is an Event`)

      const guard = EVENT_GUARDS[value.kind]
      assert.ok(guard, `${name} has a known kind`)
      assert.ok(guard(value), `${name} payload matches ${value.kind}`)
      assert.deepEqual(JSON.parse(JSON.stringify(value)), value)
      kinds.add(value.kind)
    }

    assert.deepEqual([...kinds].sort(), [...EVENT_KINDS].sort())
  })

  test("event guards reject missing fields and mismatched payloads", () => {
    const message = events["agent.message"] as Record<string, unknown>
    const { text: _text, ...withoutText } = message

    assert.equal(isEvent(withoutText), false)
    assert.equal(isChildSpawnEvent(message), false)
    assert.equal(isAgentMessageEvent({ ...message, payload: { message: "x" } }), false)
    assert.equal(isTarget({ kind: "opencode-session", id: "" }), false)
    assert.equal(isTarget({ kind: "browser", id: "x" }), false)
  })

  test("every SSE frame fixture parses", () => {
    const types = frames.map((frame) => {
      assert.ok(isSseFrame(frame), JSON.stringify(frame))
      return (frame as SseFrame).type
    })

    assert.deepEqual(types.sort(), ["event", "heartbeat", "subscribed"])
  })

  test("every kernel message fixture parses", () => {
    for (const message of kernel.to_kernel) assert.ok(isToKernelMessage(message), JSON.stringify(message))
    for (const message of kernel.from_kernel) assert.ok(isFromKernelMessage(message), JSON.stringify(message))

    assert.equal(isFromKernelMessage({ type: "host_request", id: "r1", method: "rlm.nope", params: {} }), false)
    assert.equal(isToKernelMessage({ type: "execute", id: "x", code: "1" }), false)
  })

  test("rpc fixtures use declared methods, targets, and hosts", () => {
    for (const [name, { request, response }] of Object.entries(rpc)) {
      assert.equal(response.id, request.id, name)

      if (response.error !== undefined) {
        assert.ok(isWireError(response.error), `${name} error envelope`)
        continue
      }

      assert.ok(METHODS.includes(request.method as RpcMethod), `${name} method`)
      if ("target" in request.params) assert.ok(isTarget(request.params.target), `${name} target`)
      if ("session" in request.params) assert.ok(isTarget(request.params.session), `${name} session`)
      if ("host" in request.params) assert.ok(isHostInfo(request.params.host), `${name} host`)
    }
  })
})

describe("rpc fixtures through RecurseClient", () => {
  const daemon = new FakeDaemon()
  let client: RecurseClient

  before(async () => {
    await daemon.start()
    client = new RecurseClient({ url: daemon.url })
  })

  after(() => daemon.stop())

  for (const [name, { request, response }] of Object.entries(rpc)) {
    test(name, async () => {
      daemon.on(request.method, () => {
        if (response.error !== undefined) throw new RpcFailure(response.error as RpcFailure["error"])

        return response.result
      })

      const call = client.rpc(request.method as RpcMethod, request.params as never)

      if (response.error === undefined) {
        assert.deepEqual(await call, response.result)
        assert.deepEqual(daemon.calls.at(-1)?.params, request.params)
        return
      }

      await assert.rejects(call, (error: unknown) => {
        assert.ok(error instanceof RecurseError)
        assert.deepEqual(error.toJSON(), response.error)
        return true
      })
    })
  }
})
