import { GUIDANCE_TAG, IPYTHON_DESCRIPTION, errorMessage, strictConfig } from "@recurse/client"
import { Type } from "typebox"
import { PiRecurse } from "./runtime.ts"
import type { ExtensionAPI, PiOptions } from "./sdk.ts"

export { PiRecurse } from "./runtime.ts"
export type { PiOptions, PiSdk } from "./sdk.ts"

const TOOL_NAME = "ipython"

const PARAMETERS = Type.Object({
  code: Type.String({ description: "Python source for one cell. Top-level await is allowed." }),
  timeout_sec: Type.Optional(Type.Integer({ minimum: 1, maximum: 3_600, description: "Interrupt the cell after this many seconds (default 600)." })),
})

export function createRecurseExtension(options: PiOptions = {}): (pi: ExtensionAPI) => void {
  const extension = (pi: ExtensionAPI): void => {
    const runtime = new PiRecurse(pi, { ...options, extension })
    const { strict, allow } = strictConfig(options.env ?? process.env)

    pi.registerTool({
      name: TOOL_NAME,
      label: "ipython",
      description: IPYTHON_DESCRIPTION,
      promptSnippet: "Run Python in the persistent recurse kernel: files, shell via bash(), child agents via rlm.",
      parameters: PARAMETERS,
      executionMode: "sequential",
      execute: async (_toolCallId, params, signal, _onUpdate, ctx) => {
        const input = params.timeout_sec === undefined ? { code: params.code } : { code: params.code, timeout_sec: params.timeout_sec }

        return { content: [{ type: "text", text: await runtime.execute(ctx, input, signal) }], details: undefined }
      },
    })

    pi.on("session_start", (_event, ctx) => {
      if (strict) pi.setActiveTools([TOOL_NAME, ...allow])
      void runtime.start(ctx)
    })

    pi.on("before_agent_start", async (event, ctx) => {
      void runtime.start(ctx)

      try {
        const sections = event.systemPromptOptions.sections

        if (!(GUIDANCE_TAG in sections)) sections[GUIDANCE_TAG] = await runtime.guidanceText(ctx)
      } catch (error) {
        console.error(`[recurse] guidance injection failed: ${errorMessage(error)}`)
      }
    })

    pi.on("session_shutdown", async () => {
      await runtime.shutdown()
    })
  }

  return extension
}

export default createRecurseExtension()
