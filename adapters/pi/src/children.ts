import { errorMessage, parseModel, type ChildSpawnEvent } from "@recurse/client"
import type { PiRecurse } from "./runtime.ts"
import type { AgentSession, ExtensionAPI, PiSdk } from "./sdk.ts"

interface LiveChild {
  session: AgentSession
  run: Promise<void>
}

/**
 * Runs children in-process through the Pi SDK. Each child gets its own recurse
 * extension instance (inlined, so it works even when the parent loaded recurse
 * with `-e`), which registers and subscribes the child's own target.
 */
export class PiChildren {
  private readonly live = new Map<string, LiveChild>()

  constructor(private readonly runtime: PiRecurse) {}

  async spawn(event: ChildSpawnEvent): Promise<void> {
    const { child, prompt } = event.payload

    if (this.live.has(child.child_id)) return

    const client = await this.runtime.connection.get()
    const { children } = await client.rpc("children.list", { target: event.target })
    const current = children.find((candidate) => candidate.child_id === child.child_id)

    if (current && current.status !== "pending") return

    let session: AgentSession | undefined

    try {
      session = await this.create(event)
      await client.rpc("children.bind", {
        target: event.target,
        child_id: child.child_id,
        session: { kind: "pi-session", id: session.sessionManager.getSessionId() },
      })
    } catch (error) {
      session?.dispose()
      await client.rpc("children.fail", { target: event.target, child_id: child.child_id, reason: errorMessage(error).slice(0, 1_000) })
      return
    }

    // Fire and track: the first run can take many turns; results come back as agent.message events.
    const run = session.prompt(prompt).catch((error: unknown) => {
      console.error(`[recurse] child ${child.name} run failed: ${errorMessage(error)}`)
    })

    this.live.set(child.child_id, { session, run })
  }

  async dispose(): Promise<void> {
    const live = [...this.live.values()]

    this.live.clear()
    for (const child of live) {
      try {
        child.session.dispose()
      } catch (error) {
        console.error(`[recurse] child dispose failed: ${errorMessage(error)}`)
      }
    }
  }

  private async create(event: ChildSpawnEvent): Promise<AgentSession> {
    const { child } = event.payload
    const sdk = await this.runtime.sdk()
    const ctx = this.runtime.ctx
    const parsed = parseModel(child.model)
    const model = parsed ? ctx?.modelRegistry.find(parsed.provider, parsed.model) : undefined
    const parentSession = ctx?.sessionManager.getSessionFile()
    const sessionManager = sdk.SessionManager.create
      ? sdk.SessionManager.create(child.session_dir, undefined, parentSession ? { parentSession } : {})
      : sdk.SessionManager.inMemory(child.session_dir)

    const resourceLoader = await childResources(sdk, child.session_dir, this.runtime.extension)

    const { session } = await sdk.createAgentSession({
      cwd: child.session_dir,
      sessionManager,
      ...(model ? { model } : {}),
      ...(resourceLoader ? { resourceLoader } : {}),
      sessionStartEvent: { type: "session_start", reason: "new" },
    })

    return session
  }
}

const INLINE_NAME = "recurse"

/**
 * The child's normal extensions plus exactly one recurse: the inlined factory
 * wins over any discovered extension that also registers `ipython`.
 */
async function childResources(sdk: PiSdk, cwd: string, extension: ((pi: ExtensionAPI) => void) | undefined) {
  if (!extension || !sdk.DefaultResourceLoader || !sdk.getAgentDir) return undefined

  const loader = new sdk.DefaultResourceLoader({
    cwd,
    agentDir: sdk.getAgentDir(),
    extensionFactories: [{ name: INLINE_NAME, factory: extension, hidden: true }],
    extensionsOverride: (base) => {
      const providers = base.extensions.filter((candidate) => candidate.tools.has("ipython"))
      const chosen = providers.find((candidate) => candidate.path.includes(`inline:${INLINE_NAME}`)) ?? providers.at(-1)

      return { ...base, extensions: base.extensions.filter((candidate) => !candidate.tools.has("ipython") || candidate === chosen) }
    },
  })

  await loader.reload()

  return loader
}
