import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { RecurseClient, type ClientOptions } from "./client.ts"
import { RecurseError, errorMessage, isRecurseError } from "./errors.ts"
import { daemonUrl, sleep } from "./util.ts"

const ENV_NAMES = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TERM", "TMPDIR", "TZ", "VIRTUAL_ENV"]
const ENV_PREFIXES = ["XDG_", "RECURSE_", "LC_"]
const RESPAWN_THROTTLE_MS = 15_000

let lastSpawnAt = 0

export interface EnsureDaemonOptions extends ClientOptions {
  env?: NodeJS.ProcessEnv
  /** Directory the bundled `../bin/recurse` is resolved from. Default: this module's directory. */
  moduleUrl?: string
  pollIntervalMs?: number
  startTimeoutMs?: number
}

/** Environment passed to a spawned daemon: an allowlist, never the host's full environment. */
export function daemonEnvironment(env: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const allowed: Record<string, string> = {}

  for (const [name, value] of Object.entries(env)) {
    if (value === undefined) continue
    if (ENV_NAMES.includes(name) || ENV_PREFIXES.some((prefix) => name.startsWith(prefix))) allowed[name] = value
  }

  return allowed
}

export function daemonBinary(env: NodeJS.ProcessEnv = process.env, moduleUrl: string = import.meta.url): string {
  if (env.RECURSE_BIN) return env.RECURSE_BIN

  const bundled = fileURLToPath(new URL("../bin/recurse", moduleUrl))

  return existsSync(bundled) ? bundled : "recurse"
}

/**
 * Returns a client for a healthy daemon, spawning `recurse daemon` detached when
 * nothing answers and RECURSE_DAEMON_URL is unset. A protocol mismatch is fatal.
 */
export async function ensureDaemon(options: EnsureDaemonOptions = {}): Promise<RecurseClient> {
  const env = options.env ?? process.env
  const client = new RecurseClient({ url: options.url ?? daemonUrl(env), ...tokenOption(options, env), ...timeoutOption(options) })

  try {
    await client.health({ timeoutMs: 1_000 })
    return client
  } catch (error) {
    if (!isRecurseError(error, "unavailable") && !isRecurseError(error, "timeout")) throw error
    if (env.RECURSE_DAEMON_URL || options.url) {
      throw new RecurseError("unavailable", `recurse daemon unavailable at ${client.url}: ${errorMessage(error)}`)
    }
  }

  if (Date.now() - lastSpawnAt >= RESPAWN_THROTTLE_MS) {
    lastSpawnAt = Date.now()
    launch(daemonBinary(env, options.moduleUrl), env)
  }

  const deadline = Date.now() + (options.startTimeoutMs ?? 10_000)
  let last: unknown

  while (Date.now() < deadline) {
    await sleep(options.pollIntervalMs ?? 150)
    try {
      await client.health({ timeoutMs: 1_000 })
      return client
    } catch (error) {
      if (isRecurseError(error, "protocol_mismatch")) throw error

      last = error
    }
  }

  throw new RecurseError("unavailable", `recurse daemon did not become healthy at ${client.url}: ${errorMessage(last)}`)
}

/** Test hook: forget the last spawn so the throttle does not leak between tests. */
export function resetSpawnThrottle(): void {
  lastSpawnAt = 0
}

function launch(binary: string, env: NodeJS.ProcessEnv): void {
  try {
    const child = spawn(binary, ["daemon"], { detached: true, stdio: "ignore", env: daemonEnvironment(env) })

    child.on("error", (error) => console.error(`[recurse] could not start ${binary} daemon: ${error.message}`))
    child.unref()
  } catch (error) {
    console.error(`[recurse] could not start ${binary} daemon: ${errorMessage(error)}`)
  }
}

function tokenOption(options: ClientOptions, env: NodeJS.ProcessEnv): Pick<ClientOptions, "token"> {
  const token = options.token ?? env.RECURSE_DAEMON_TOKEN

  return token === undefined ? {} : { token }
}

function timeoutOption(options: ClientOptions): Pick<ClientOptions, "timeoutMs"> {
  return options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }
}
