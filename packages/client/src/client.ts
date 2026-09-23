import http from "node:http"
import https from "node:https"
import { RecurseError, errorMessage } from "./errors.ts"
import {
  arrayOf,
  isCellResult,
  isChildInfo,
  isEvent,
  isHealth,
  isKernelStatus,
  isRecord,
  isRegisterResult,
  isSkillInfo,
  isString,
  isWireError,
} from "./guards.ts"
import { PROTOCOL_VERSION, type Health, type RpcMethod, type RpcParams, type RpcResult, type Target } from "./protocol.ts"
import { subscribe, type EventHandler, type SubscribeOptions, type Subscription } from "./subscribe.ts"
import { daemonUrl } from "./util.ts"

export interface ClientOptions {
  url?: string
  token?: string
  /** Default RPC timeout; `kernel.execute` derives its own from `timeout_sec`. */
  timeoutMs?: number
}

export interface RpcOptions {
  signal?: AbortSignal
  timeoutMs?: number
}

const DEFAULT_TIMEOUT_MS = 30_000
const MAX_CELL_TIMEOUT_SEC = 3_600

const hasChild = (value: unknown): boolean => isRecord(value) && isChildInfo(value.child)

const RESULT_GUARDS: { [M in RpcMethod]?: (value: unknown) => boolean } = {
  health: isHealth,
  "target.register": isRegisterResult,
  "kernel.execute": isCellResult,
  "kernel.status": isKernelStatus,
  "children.list": (value) => isRecord(value) && arrayOf(isChildInfo)(value.children),
  "children.bind": hasChild,
  "children.fail": hasChild,
  "children.delete": hasChild,
  "events.list": (value) => isRecord(value) && arrayOf(isEvent)(value.events),
  "skills.list": (value) => isRecord(value) && arrayOf(isSkillInfo)(value.skills),
}

export class RecurseClient {
  readonly url: string
  private readonly token: string | undefined
  private readonly timeoutMs: number
  private nextId = 1

  constructor(options: ClientOptions = {}) {
    this.url = (options.url ?? daemonUrl()).replace(/\/+$/, "")
    this.token = options.token ?? process.env.RECURSE_DAEMON_TOKEN
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS
  }

  headers(): Record<string, string> {
    return this.token ? { authorization: `Bearer ${this.token}` } : {}
  }

  async rpc<M extends RpcMethod>(method: M, params: RpcParams<M>, options: RpcOptions = {}): Promise<RpcResult<M>> {
    const id = this.nextId++
    const body = JSON.stringify({ id, method, params })
    const timeoutMs = options.timeoutMs ?? this.defaultTimeout(method, params)
    const { status, text } = await this.post(body, timeoutMs, options.signal)

    let envelope: unknown
    try {
      envelope = JSON.parse(text)
    } catch {
      throw new RecurseError(status === 401 ? "unauthorized" : "invalid_response", `daemon returned HTTP ${status} with a non-JSON body`, status)
    }

    if (!isRecord(envelope)) throw new RecurseError("invalid_response", "daemon response is not an object", status)
    if (envelope.error !== undefined) throw wireError(envelope.error, status)
    if (!("result" in envelope)) throw new RecurseError("invalid_response", `daemon response to ${method} has no result`, status)

    const guard = RESULT_GUARDS[method]
    if (guard && !guard(envelope.result)) {
      throw new RecurseError("invalid_response", `daemon returned a malformed ${method} result`, status)
    }

    return envelope.result as RpcResult<M>
  }

  /** `health` plus the protocol check every client must make before talking to a daemon. */
  async health(options: RpcOptions = {}): Promise<Health> {
    const health = await this.rpc("health", {}, options)

    if (health.protocol !== PROTOCOL_VERSION) {
      throw new RecurseError(
        "protocol_mismatch",
        `recurse daemon at ${this.url} speaks protocol ${health.protocol}; this client speaks ${PROTOCOL_VERSION}`,
      )
    }

    return health
  }

  subscribe(target: Target, handler: EventHandler, options: SubscribeOptions = {}): Promise<Subscription> {
    return subscribe(this, target, handler, options)
  }

  private defaultTimeout(method: RpcMethod, params: unknown): number {
    if (method !== "kernel.execute") return this.timeoutMs

    const requested = isRecord(params) && typeof params.timeout_sec === "number" ? params.timeout_sec : MAX_CELL_TIMEOUT_SEC

    // The daemon interrupts at timeout_sec and may wait 5 s more before killing the kernel.
    return Math.min(requested, MAX_CELL_TIMEOUT_SEC) * 1_000 + 30_000
  }

  // node:http rather than fetch: undici's 300 s headers timeout would cut off long cells.
  private post(body: string, timeoutMs: number, signal?: AbortSignal): Promise<{ status: number; text: string }> {
    const target = new URL(`${this.url}/rpc`)
    const transport = target.protocol === "https:" ? https : http

    return new Promise((resolve, reject) => {
      if (signal?.aborted) return reject(new RecurseError("unavailable", "request aborted"))

      const request = transport.request(target, {
        method: "POST",
        headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body), ...this.headers() },
      })
      const timer = setTimeout(() => request.destroy(new RecurseError("timeout", `daemon did not answer within ${timeoutMs} ms`)), timeoutMs)
      const onAbort = (): void => {
        request.destroy(new RecurseError("unavailable", "request aborted"))
      }
      const finish = (): void => {
        clearTimeout(timer)
        signal?.removeEventListener("abort", onAbort)
      }

      signal?.addEventListener("abort", onAbort, { once: true })
      request.on("error", (error) => {
        finish()
        reject(error instanceof RecurseError ? error : new RecurseError("unavailable", `recurse daemon unreachable at ${this.url}: ${errorMessage(error)}`))
      })
      request.on("response", (response) => {
        const chunks: Buffer[] = []

        response.on("data", (chunk: Buffer) => chunks.push(chunk))
        response.on("error", (error) => {
          finish()
          reject(new RecurseError("unavailable", `daemon response failed: ${errorMessage(error)}`))
        })
        response.on("end", () => {
          finish()
          resolve({ status: response.statusCode ?? 0, text: Buffer.concat(chunks).toString("utf8") })
        })
      })
      request.end(body)
    })
  }
}

function wireError(value: unknown, status: number): RecurseError {
  if (isWireError(value)) return new RecurseError(value.code, value.message, status)
  if (isRecord(value) && isString(value.message)) return new RecurseError("internal", value.message, status)

  return new RecurseError("internal", `daemon error: ${JSON.stringify(value)}`, status)
}
