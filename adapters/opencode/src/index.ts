import { Plugin } from "@opencode/plugin"
import { errorMessage, loadGuidance } from "@recurse/client"
import { RecurseBridge } from "./bridge.ts"
import { contextHook } from "./context.ts"
import { ipythonTool } from "./tool.ts"
import type { AdapterOptions, HostContext } from "./types.ts"

export { RecurseBridge } from "./bridge.ts"
export type { AdapterOptions, HostContext } from "./types.ts"

export async function setupRecurse(ctx: HostContext, options: AdapterOptions = {}): Promise<() => Promise<void>> {
  const env = options.env ?? process.env
  const bridge = new RecurseBridge(ctx, options)
  const guidance = options.guidance ?? loadGuidance(import.meta.url)

  const tools = await ctx.tool.transform((editor) => {
    editor.add(ipythonTool(bridge))
  })
  const context = await ctx.session.hook("context", contextHook(bridge, guidance, env))
  // Subscribe on the first prompt, not only on the first tool call, so events reach idle sessions.
  const prompt = await ctx.session.hook("prompt", (event) => {
    void bridge.track(String(event.sessionID))
  })

  void bridge
    .daemon()
    .then(() => bridge.restore())
    .catch((error: unknown) => console.error(`[recurse] startup: ${errorMessage(error)}`))

  return async () => {
    await Promise.allSettled([tools.dispose(), context.dispose(), prompt.dispose()])
    await bridge.close()
  }
}

export default Plugin.define({
  id: "recurse",
  setup: (ctx) => setupRecurse(ctx),
})
