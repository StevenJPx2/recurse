import { RecurseError, errorMessage } from "./errors.ts"
import { isSseFrame } from "./guards.ts"
import type { RecurseEvent, RpcMethod, RpcParams, RpcResult, SseFrame, Target } from "./protocol.ts"
import { Backoff, BoundedSet, encodeTarget, sleep, targetKey } from "./util.ts"

export type EventHandler = (event: RecurseEvent) => Promise<void> | void

export interface SubscribeOptions {
  signal?: AbortSignal
  /** How long to wait for the first `subscribed` frame. Default 15 s. */
  subscribeTimeoutMs?: number
  /** Reconnect when no frame arrives for this long. Default 45 s (three missed heartbeats). */
  heartbeatTimeoutMs?: number
  backoff?: { initialMs?: number; maxMs?: number }
  onError?: (error: unknown) => void
}

export interface Subscription {
  readonly target: Target
  readonly closed: boolean
  close(): void
}

interface StreamHost {
  readonly url: string
  headers(): Record<string, string>
  rpc<M extends RpcMethod>(method: M, params: RpcParams<M>, options?: { signal?: AbortSignal }): Promise<RpcResult<M>>
}

const MAX_BUFFER = 2 * 1024 * 1024

export async function subscribe(host: StreamHost, target: Target, handler: EventHandler, options: SubscribeOptions = {}): Promise<Subscription> {
  const stream = new EventStream(host, target, handler, options)

  try {
    await stream.start()
  } catch (error) {
    stream.close()
    throw error
  }

  return stream
}

class EventStream implements Subscription {
  private readonly controller = new AbortController()
  private readonly backoff: Backoff
  private readonly ackBackoff: Backoff
  private readonly inflight = new Set<string>()
  private readonly acked = new BoundedSet(1_024)
  private connection: AbortController | undefined
  private chain: Promise<void> = Promise.resolve()
  private handlerFailed = false
  private failures = 0
  private ready: { resolve(): void; reject(error: unknown): void } | undefined

  constructor(
    private readonly host: StreamHost,
    readonly target: Target,
    private readonly handler: EventHandler,
    private readonly options: SubscribeOptions,
  ) {
    this.backoff = new Backoff(options.backoff?.initialMs ?? 500, options.backoff?.maxMs ?? 10_000)
    this.ackBackoff = new Backoff(options.backoff?.initialMs ?? 500, options.backoff?.maxMs ?? 10_000)
    options.signal?.addEventListener("abort", () => this.close(), { once: true })
  }

  get closed(): boolean {
    return this.controller.signal.aborted
  }

  start(): Promise<void> {
    const timeoutMs = this.options.subscribeTimeoutMs ?? 15_000
    const ready = new Promise<void>((resolve, reject) => {
      this.ready = { resolve, reject }
    })
    const timer = setTimeout(() => {
      this.ready?.reject(new RecurseError("timeout", `recurse event stream for ${targetKey(this.target)} did not subscribe within ${timeoutMs} ms`))
    }, timeoutMs)

    if (this.options.signal?.aborted) this.close()
    void this.loop()

    return ready.finally(() => {
      clearTimeout(timer)
      this.ready = undefined
    })
  }

  close(): void {
    if (this.closed) return

    this.controller.abort()
    this.connection?.abort()
    this.ready?.reject(new RecurseError("unavailable", "subscription closed"))
  }

  private async loop(): Promise<void> {
    while (!this.closed) {
      const connection = new AbortController()

      this.connection = connection
      try {
        await this.connect(connection)
      } catch (error) {
        if (!this.closed && !connection.signal.aborted) this.report(error)
      } finally {
        connection.abort()
      }

      if (this.closed) break

      await sleep(this.backoff.next(), this.controller.signal)
    }
  }

  private async connect(connection: AbortController): Promise<void> {
    const url = `${this.host.url}/events?target=${encodeTarget(this.target)}`
    const response = await fetch(url, {
      headers: { accept: "text/event-stream", ...this.host.headers() },
      signal: connection.signal,
    })

    if (!response.ok || !response.body) {
      await response.body?.cancel()
      throw new RecurseError(response.status === 401 ? "unauthorized" : "unavailable", `event stream failed: HTTP ${response.status}`, response.status)
    }

    const heartbeatMs = this.options.heartbeatTimeoutMs ?? 45_000
    const seen = new Set<string>()
    let watchdog = setTimeout(() => connection.abort(), heartbeatMs)
    const rearm = (): void => {
      clearTimeout(watchdog)
      watchdog = setTimeout(() => connection.abort(), heartbeatMs)
    }

    const reader = response.body.getReader()

    try {
      const decoder = new TextDecoder()
      let buffer = ""

      while (true) {
        const { done, value } = await reader.read()
        if (done) break

        buffer += decoder.decode(value, { stream: true })

        const blocks = buffer.split(/\r?\n\r?\n/)
        buffer = blocks.pop() ?? ""
        if (buffer.length > MAX_BUFFER) throw new RecurseError("invalid_response", "SSE frame exceeds the protocol bound")

        for (const block of blocks) {
          const frame = parseBlock(block)
          if (!frame) continue

          rearm()
          this.onFrame(frame, seen)
        }
      }
    } finally {
      clearTimeout(watchdog)
    }
  }

  private onFrame(frame: SseFrame, seen: Set<string>): void {
    if (frame.type === "subscribed") {
      this.failures = 0
      if (!this.handlerFailed) this.backoff.reset()
      this.ready?.resolve()
      return
    }

    if (frame.type !== "event") return

    for (const event of frame.events) {
      if (seen.has(event.id) || this.inflight.has(event.id) || this.acked.has(event.id)) continue

      seen.add(event.id)
      this.inflight.add(event.id)
      this.chain = this.chain.then(() => this.deliver(event))
    }
  }

  private async deliver(event: RecurseEvent): Promise<void> {
    if (this.closed) return

    try {
      await this.handler(event)
    } catch (error) {
      // No ack: drop the connection so the daemon replays the event on reconnect.
      this.inflight.delete(event.id)
      this.handlerFailed = true
      this.report(new Error(`handler failed for ${event.id}: ${errorMessage(error)}`))
      this.connection?.abort()
      return
    }

    this.handlerFailed = false
    await this.ack(event.id)
    this.inflight.delete(event.id)
    this.acked.add(event.id)
  }

  private async ack(eventId: string): Promise<void> {
    while (!this.closed) {
      try {
        await this.host.rpc("events.ack", { target: this.target, event_ids: [eventId] }, { signal: this.controller.signal })
        this.ackBackoff.reset()
        return
      } catch (error) {
        if (this.closed) return

        this.report(new Error(`ack failed for ${eventId}: ${errorMessage(error)}`))
        await sleep(this.ackBackoff.next(), this.controller.signal)
      }
    }
  }

  private report(error: unknown): void {
    this.failures += 1

    if (this.options.onError) return this.options.onError(error)
    // Only the first failure of a burst; the reconnect loop is expected to recover.
    if (this.failures === 1) console.error(`[recurse] ${targetKey(this.target)}: ${errorMessage(error)}`)
  }
}

function parseBlock(block: string): SseFrame | undefined {
  const data = block
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(line.startsWith("data: ") ? 6 : 5))
    .join("\n")

  if (!data) return undefined

  try {
    const frame: unknown = JSON.parse(data)

    return isSseFrame(frame) ? frame : undefined
  } catch {
    return undefined
  }
}
