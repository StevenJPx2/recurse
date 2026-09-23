import type { Target } from "./protocol.ts"

export const DEFAULT_DAEMON_URL = "http://127.0.0.1:18791"

export function daemonUrl(env: NodeJS.ProcessEnv = process.env): string {
  if (env.RECURSE_DAEMON_URL) return env.RECURSE_DAEMON_URL.replace(/\/+$/, "")
  if (env.RECURSE_DAEMON_PORT) return `http://127.0.0.1:${env.RECURSE_DAEMON_PORT}`

  return DEFAULT_DAEMON_URL
}

export function targetKey(target: Target): string {
  return `${target.kind}:${target.id}`
}

export function encodeTarget(target: Target): string {
  return Buffer.from(JSON.stringify({ kind: target.kind, id: target.id }), "utf8").toString("base64url")
}

/** Splits a `provider/model` reference at the first slash (model IDs may contain more). */
export function parseModel(model: string | null | undefined): { provider: string; model: string } | undefined {
  if (!model) return undefined

  const slash = model.indexOf("/")
  if (slash <= 0 || slash === model.length - 1) return undefined

  return { provider: model.slice(0, slash), model: model.slice(slash + 1) }
}

/** Resolves after `ms`, or early (without rejecting) when `signal` aborts. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted) return resolve()

    const done = (): void => {
      clearTimeout(timer)
      signal?.removeEventListener("abort", done)
      resolve()
    }
    const timer = setTimeout(done, ms)

    signal?.addEventListener("abort", done, { once: true })
  })
}

export class Backoff {
  private current: number

  constructor(
    private readonly initial = 500,
    private readonly max = 10_000,
  ) {
    this.current = initial
  }

  next(): number {
    const delay = this.current

    this.current = Math.min(this.current * 2, this.max)
    return delay
  }

  reset(): void {
    this.current = this.initial
  }
}

/** Insertion-ordered set that forgets its oldest entries past `limit`. */
export class BoundedSet {
  private readonly items = new Set<string>()

  constructor(private readonly limit: number) {}

  has(value: string): boolean {
    return this.items.has(value)
  }

  add(value: string): void {
    this.items.delete(value)
    this.items.add(value)

    for (const oldest of this.items) {
      if (this.items.size <= this.limit) break

      this.items.delete(oldest)
    }
  }

  delete(value: string): void {
    this.items.delete(value)
  }
}
