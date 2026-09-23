import http from "node:http"
import type { AddressInfo } from "node:net"
import type { RecurseEvent, Target, WireError } from "../../src/protocol.ts"

export type Handler = (params: Record<string, unknown>) => unknown

export interface RpcCall {
  method: string
  params: Record<string, unknown>
}

export class RpcFailure {
  constructor(readonly error: WireError) {}
}

/** In-process stand-in for `recurse daemon`: RPC, replay-until-ack SSE, and knobs for failure modes. */
export class FakeDaemon {
  readonly calls: RpcCall[] = []
  readonly handlers = new Map<string, Handler>()
  readonly queues = new Map<string, RecurseEvent[]>()
  readonly streams = new Set<{ key: string; response: http.ServerResponse }>()
  subscribeCount = 0
  protocol = 1
  token: string | undefined
  /** When false, SSE connections get no frames at all (heartbeat watchdog tests). */
  sendFrames = true
  private server: http.Server | undefined

  url = ""

  async start(): Promise<this> {
    this.server = http.createServer((request, response) => void this.route(request, response))
    await new Promise<void>((resolve) => this.server!.listen(0, "127.0.0.1", resolve))
    this.url = `http://127.0.0.1:${(this.server!.address() as AddressInfo).port}`
    return this
  }

  async stop(): Promise<void> {
    this.dropStreams()
    this.server?.closeAllConnections()
    await new Promise<void>((resolve) => this.server?.close(() => resolve()) ?? resolve())
  }

  on(method: string, handler: Handler): this {
    this.handlers.set(method, handler)
    return this
  }

  callsTo(method: string): RpcCall[] {
    return this.calls.filter((call) => call.method === method)
  }

  queued(target: Target): RecurseEvent[] {
    return this.queues.get(key(target)) ?? []
  }

  push(event: RecurseEvent): void {
    const k = key(event.target)

    this.queues.set(k, [...(this.queues.get(k) ?? []), event])
    for (const stream of this.streams) {
      if (stream.key === k && this.sendFrames) write(stream.response, { type: "event", target: event.target, events: [event] })
    }
  }

  heartbeat(): void {
    for (const stream of this.streams) write(stream.response, { type: "heartbeat", at: Date.now() })
  }

  dropStreams(): void {
    for (const stream of this.streams) stream.response.destroy()
    this.streams.clear()
  }

  private async route(request: http.IncomingMessage, response: http.ServerResponse): Promise<void> {
    if (this.token && request.headers.authorization !== `Bearer ${this.token}`) {
      response.writeHead(401, { "content-type": "application/json" })
      response.end(JSON.stringify({ error: { code: "unauthorized", message: "bad token" } }))
      return
    }

    const url = new URL(request.url ?? "/", this.url)

    if (request.method === "GET" && url.pathname === "/events") return this.events(url, response)
    if (request.method === "POST" && url.pathname === "/rpc") return this.rpc(request, response)

    response.writeHead(404).end()
  }

  private events(url: URL, response: http.ServerResponse): void {
    const target = JSON.parse(Buffer.from(url.searchParams.get("target") ?? "", "base64url").toString("utf8")) as Target
    const stream = { key: key(target), response }

    this.subscribeCount += 1
    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" })
    response.flushHeaders()
    this.streams.add(stream)
    response.on("close", () => this.streams.delete(stream))

    if (!this.sendFrames) return

    write(response, { type: "subscribed", target })
    write(response, { type: "event", target, events: this.queued(target) })
  }

  private async rpc(request: http.IncomingMessage, response: http.ServerResponse): Promise<void> {
    const chunks: Buffer[] = []

    for await (const chunk of request) chunks.push(chunk as Buffer)

    const body = JSON.parse(Buffer.concat(chunks).toString("utf8")) as { id?: number; method: string; params?: Record<string, unknown> }
    const params = body.params ?? {}

    this.calls.push({ method: body.method, params })

    try {
      const result = await this.dispatch(body.method, params)

      response.writeHead(200, { "content-type": "application/json" })
      response.end(JSON.stringify({ id: body.id, result }))
    } catch (error) {
      const wire = error instanceof RpcFailure ? error.error : { code: "internal", message: String(error) }

      response.writeHead(400, { "content-type": "application/json" })
      response.end(JSON.stringify({ id: body.id, error: wire }))
    }
  }

  private async dispatch(method: string, params: Record<string, unknown>): Promise<unknown> {
    const handler = this.handlers.get(method)
    if (handler) return handler(params)

    if (method === "health") {
      return { ok: true, name: "recurse", version: "0.1.0", protocol: this.protocol, pid: process.pid, started_at: 1 }
    }
    if (method === "events.ack") {
      const k = key(params.target as Target)
      const ids = new Set(params.event_ids as string[])
      const before = this.queues.get(k) ?? []
      const after = before.filter((event) => !ids.has(event.id))

      this.queues.set(k, after)
      return { acked: before.length - after.length }
    }
    if (method === "target.register") {
      return { target: params.target, depth: 0, parent: null, child: null }
    }
    if (method === "skills.list") return { skills: [] }

    throw new RpcFailure({ code: "invalid_request", message: `unknown method ${method}` })
  }
}

export function key(target: Target): string {
  return `${target.kind}:${target.id}`
}

function write(response: http.ServerResponse, frame: unknown): void {
  response.write(`data: ${JSON.stringify(frame)}\n\n`)
}

export function makeEvent(target: Target, id: string, overrides: Partial<RecurseEvent> = {}): RecurseEvent {
  return {
    id,
    target,
    kind: "agent.message",
    actionable: true,
    at: Date.now(),
    text: `[recurse] ${id}`,
    payload: { from: { role: "child", name: "c", child_id: "c_1" }, message: id },
    ...overrides,
  }
}

export async function waitFor(check: () => boolean, timeoutMs = 3_000, label = "condition"): Promise<void> {
  const deadline = Date.now() + timeoutMs

  while (!check()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)

    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
