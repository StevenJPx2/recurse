import { GUIDANCE_TAG, errorMessage, renderGuidance, strictConfig } from "@recurse/client"
import type { SessionContext } from "@opencode/plugin/promise/session"
import type { RecurseBridge } from "./bridge.ts"
import { TOOL_NAME } from "./tool.ts"

/** Guidance + skill listing into the system prompt, and strict-mode tool pruning. Never throws. */
export function contextHook(bridge: RecurseBridge, guidance: string, env: NodeJS.ProcessEnv) {
  const { strict, allow } = strictConfig(env)
  const keep = new Set([TOOL_NAME, ...allow])

  return async (event: SessionContext): Promise<void> => {
    const sessionID = String(event.sessionID)

    try {
      bridge.noteSession(sessionID, `${event.model.providerID}/${event.model.id}`, String(event.agent))

      if (!event.system.some((part) => part.text.includes(`<${GUIDANCE_TAG}>`))) {
        event.system.push({ type: "text", text: renderGuidance(guidance, await bridge.skills(sessionID)) })
      }
    } catch (error) {
      console.error(`[recurse] guidance injection failed: ${errorMessage(error)}`)
    }

    if (!strict) return

    for (const name of Object.keys(event.tools)) {
      if (!keep.has(name)) delete event.tools[name]
    }
  }
}
