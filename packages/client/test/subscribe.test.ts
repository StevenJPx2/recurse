import assert from "node:assert/strict"
import { afterEach, beforeEach, describe, test } from "node:test"
import { RecurseClient, RecurseError, type RecurseEvent, type SubscribeOptions, type Subscription } from "../src/index.ts"
import { FakeDaemon, RpcFailure, makeEvent, waitFor } from "./helpers/fake-daemon.ts"

const target = { kind: "opencode-session", id: "ses_abc" } as const
const fast: SubscribeOptions = { backoff: { initialMs: 20, maxMs: 50 }, onError: () => {} }

describe("RecurseClient.subscribe", () => {
  let daemon: FakeDaemon
  let client: RecurseClient
  let subscription: Subscription | undefined

  beforeEach(async () => {
    daemon = await new FakeDaemon().start()
    client = new RecurseClient({ url: daemon.url })
  })

  afterEach(async () => {
    subscription?.close()
    subscription = undefined
    await daemon.stop()
  })

  test("replays queued events, then delivers live ones, acking each after its handler", async () => {
    const seen: string[] = []
    daemon.push(makeEvent(target, "e1"))
    daemon.push(makeEvent(target, "e2"))

    subscription = await client.subscribe(target, (event) => void seen.push(event.id), fast)
    await waitFor(() => daemon.queued(target).length === 0, 3_000, "replay acked")

    daemon.push(makeEvent(target, "e3"))
    await waitFor(() => daemon.queued(target).length === 0 && seen.length === 3, 3_000, "live acked")

    assert.deepEqual(seen, ["e1", "e2", "e3"])
    assert.deepEqual(daemon.callsTo("events.ack").map((call) => call.params.event_ids), [["e1"], ["e2"], ["e3"]])
  })

  test("acks only after the handler resolves", async () => {
    let release!: () => void
    const gate = new Promise<void>((resolve) => (release = resolve))
    let started = false

    subscription = await client.subscribe(target, async () => {
      started = true
      await gate
    }, fast)
    daemon.push(makeEvent(target, "slow"))

    await waitFor(() => started, 3_000, "handler start")
    await new Promise((resolve) => setTimeout(resolve, 50))
    assert.equal(daemon.callsTo("events.ack").length, 0)

    release()
    await waitFor(() => daemon.callsTo("events.ack").length === 1, 3_000, "ack")
  })

  test("a throwing handler is not acked and gets the event again after reconnect", async () => {
    const attempts: string[] = []
    daemon.push(makeEvent(target, "flaky"))

    subscription = await client.subscribe(target, (event: RecurseEvent) => {
      attempts.push(event.id)
      if (attempts.length === 1) throw new Error("host busy")
    }, fast)

    await waitFor(() => daemon.queued(target).length === 0, 3_000, "redelivered and acked")
    assert.deepEqual(attempts, ["flaky", "flaky"])
    assert.ok(daemon.subscribeCount >= 2)
    assert.equal(daemon.callsTo("events.ack").length, 1)
  })

  test("dedupes an event id repeated within one connection", async () => {
    const seen: string[] = []
    let release!: () => void
    const gate = new Promise<void>((resolve) => (release = resolve))

    subscription = await client.subscribe(target, async (event) => {
      seen.push(event.id)
      await gate
    }, fast)
    daemon.push(makeEvent(target, "dup"))
    daemon.push(makeEvent(target, "dup"))
    release()

    await waitFor(() => daemon.queued(target).length === 0, 3_000, "acked")
    assert.deepEqual(seen, ["dup"])
  })

  test("retries a failed ack without re-running the handler", async () => {
    let handled = 0
    let ackFailures = 1
    daemon.on("events.ack", (params) => {
      if (ackFailures-- > 0) throw new RpcFailure({ code: "internal", message: "flaky" })

      daemon.queues.set("opencode-session:ses_abc", [])
      return { acked: (params.event_ids as string[]).length }
    })
    daemon.push(makeEvent(target, "once"))

    subscription = await client.subscribe(target, () => void handled++, fast)

    await waitFor(() => daemon.callsTo("events.ack").length === 2, 3_000, "ack retry")
    assert.equal(handled, 1)
  })

  test("reconnects when the heartbeat watchdog fires", async () => {
    subscription = await client.subscribe(target, () => {}, { ...fast, heartbeatTimeoutMs: 150 })

    await waitFor(() => daemon.subscribeCount >= 2, 3_000, "watchdog reconnect")
  })

  test("heartbeats keep a quiet connection alive", async () => {
    const timer = setInterval(() => daemon.heartbeat(), 40)
    try {
      subscription = await client.subscribe(target, () => {}, { ...fast, heartbeatTimeoutMs: 150 })
      await new Promise((resolve) => setTimeout(resolve, 400))
      assert.equal(daemon.subscribeCount, 1)
    } finally {
      clearInterval(timer)
    }
  })

  test("reconnects after the daemon drops the stream and replays unacked events", async () => {
    const seen: string[] = []
    subscription = await client.subscribe(target, (event) => void seen.push(event.id), fast)

    daemon.dropStreams()
    daemon.queues.set("opencode-session:ses_abc", [makeEvent(target, "while-away")])

    await waitFor(() => seen.includes("while-away"), 3_000, "replay after reconnect")
    assert.ok(daemon.subscribeCount >= 2)
  })

  test("rejects when no `subscribed` frame arrives in time", async () => {
    daemon.sendFrames = false

    await assert.rejects(
      client.subscribe(target, () => {}, { ...fast, subscribeTimeoutMs: 150 }),
      (error: unknown) => error instanceof RecurseError && error.code === "timeout",
    )
  })

  test("close() and an aborted signal stop reconnecting", async () => {
    const controller = new AbortController()
    subscription = await client.subscribe(target, () => {}, { ...fast, signal: controller.signal })

    controller.abort()
    assert.equal(subscription.closed, true)

    const count = daemon.subscribeCount
    daemon.dropStreams()
    await new Promise((resolve) => setTimeout(resolve, 150))
    assert.equal(daemon.subscribeCount, count)
  })
})
