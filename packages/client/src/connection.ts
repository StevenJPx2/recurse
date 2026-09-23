import type { RecurseClient } from "./client.ts"
import { RecurseError, errorMessage, isRecurseError } from "./errors.ts"
import { ensureDaemon } from "./spawn.ts"

/**
 * One lazily connected client per adapter. After a failed attempt, calls fail
 * fast for `retryMs` so a missing daemon never stalls every tool call and hook.
 */
export class DaemonConnection {
  private client: RecurseClient | undefined
  private connecting: Promise<RecurseClient> | undefined
  private failedAt = 0
  private lastError = "not started"

  constructor(
    private readonly connect: () => Promise<RecurseClient> = () => ensureDaemon(),
    private readonly retryMs = 15_000,
  ) {}

  get current(): RecurseClient | undefined {
    return this.client
  }

  async get(): Promise<RecurseClient> {
    if (this.client) return this.client
    if (this.connecting) return this.connecting
    if (Date.now() - this.failedAt < this.retryMs) {
      throw new RecurseError("unavailable", `recurse daemon unavailable: ${this.lastError}`)
    }

    this.connecting = this.connect()
      .then((client) => {
        this.client = client
        return client
      })
      .catch((error: unknown) => {
        this.failedAt = Date.now()
        this.lastError = errorMessage(error)
        console.error(`[recurse] daemon unavailable: ${this.lastError}`)
        throw error
      })
      .finally(() => {
        this.connecting = undefined
      })

    return this.connecting
  }

  /** Drop the client after a transport failure so the next call respawns the daemon. */
  forget(error: unknown): void {
    if (isRecurseError(error, "unavailable")) this.client = undefined
  }
}
