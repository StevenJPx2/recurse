import { IPYTHON_DESCRIPTION, formatCell, formatFailure, isRecord, isRecurseError } from "@recurse/client"
import type { ToolContext } from "@opencode/plugin/promise/tool"
import type { RecurseBridge } from "./bridge.ts"

export const TOOL_NAME = "ipython"

const INPUT = {
  type: "object",
  properties: {
    code: { type: "string", description: "Python source for one cell. Top-level await is allowed." },
    timeout_sec: { type: "integer", minimum: 1, maximum: 3_600, description: "Interrupt the cell after this many seconds (default 600)." },
  },
  required: ["code"],
  additionalProperties: false,
} as const

export function ipythonTool(bridge: RecurseBridge) {
  return {
    name: TOOL_NAME,
    description: IPYTHON_DESCRIPTION,
    input: INPUT,
    // A direct tool: strict mode removes Code Mode's `execute`, so a catalog entry would be unreachable.
    options: { codemode: false },
    async execute(input: unknown, context: ToolContext) {
      return { content: await runCell(bridge, String(context.sessionID), input, context.signal, String(context.agent)) }
    },
  }
}

/** Runs one cell and always resolves to model-facing text; host tools must not throw. */
export async function runCell(bridge: RecurseBridge, sessionID: string, input: unknown, signal?: AbortSignal, agent?: string): Promise<string> {
  const params = parseInput(input)
  if (typeof params === "string") return params

  bridge.noteSession(sessionID, undefined, agent)

  try {
    const client = await bridge.daemon()
    const target = bridge.target(sessionID)
    const interrupt = (): void => void client.rpc("kernel.interrupt", { target }).catch(() => {})

    await bridge.register(sessionID)
    void bridge.track(sessionID)
    signal?.addEventListener("abort", interrupt, { once: true })

    try {
      return formatCell(await client.rpc("kernel.execute", { target, ...params }))
    } catch (error) {
      if (!isRecurseError(error, "not_found")) throw error

      // The daemon lost the registration (e.g. its state was reset): register again once.
      await bridge.register(sessionID, true)
      return formatCell(await client.rpc("kernel.execute", { target, ...params }))
    } finally {
      signal?.removeEventListener("abort", interrupt)
    }
  } catch (error) {
    bridge.forget(error)
    return formatFailure(error)
  }
}

function parseInput(input: unknown): { code: string; timeout_sec?: number } | string {
  if (!isRecord(input) || typeof input.code !== "string") return "[invalid input] ipython needs a `code` string."

  const timeout = input.timeout_sec
  if (timeout === undefined || timeout === null) return { code: input.code }
  if (typeof timeout !== "number" || !Number.isFinite(timeout) || timeout <= 0) {
    return "[invalid input] timeout_sec must be a positive number of seconds."
  }

  return { code: input.code, timeout_sec: Math.min(Math.ceil(timeout), 3_600) }
}
