import { errorMessage, parseModel, type ChildSpawnEvent } from "@recurse/client"
import type { RecurseBridge } from "./bridge.ts"
import { TARGET_KIND } from "./types.ts"

/**
 * Turns `child.spawn` into a native OpenCode session. Redelivery is safe: the
 * daemon's child status says whether a session was already bound, and
 * `unprompted` covers a bind that succeeded before the first prompt failed.
 */
export class ChildSpawner {
  private readonly created = new Map<string, string>()
  private readonly unprompted = new Set<string>()

  constructor(private readonly bridge: RecurseBridge) {}

  async spawn(event: ChildSpawnEvent): Promise<void> {
    const { child, prompt } = event.payload
    const parent = event.target
    const client = await this.bridge.daemon()
    const { children } = await client.rpc("children.list", { target: parent })
    const current = children.find((candidate) => candidate.child_id === child.child_id)

    if (current && current.status !== "pending") {
      if (current.status === "running" && current.session && this.unprompted.has(child.child_id)) {
        await this.prompt(child.child_id, current.session.id, prompt)
      }
      return
    }

    let sessionID: string

    try {
      sessionID = await this.createSession(event)
      await client.rpc("children.bind", {
        target: parent,
        child_id: child.child_id,
        session: { kind: TARGET_KIND, id: sessionID },
      })
    } catch (error) {
      // children.fail itself throwing leaves the event unacked for redelivery.
      await client.rpc("children.fail", { target: parent, child_id: child.child_id, reason: errorMessage(error).slice(0, 1_000) })
      return
    }

    this.unprompted.add(child.child_id)
    await this.bridge.track(sessionID)
    await this.prompt(child.child_id, sessionID, prompt)
  }

  private async createSession(event: ChildSpawnEvent): Promise<string> {
    const { child } = event.payload
    const existing = this.created.get(child.child_id)

    if (existing) return existing

    const parentID = event.target.id
    const parent = await this.bridge.ctx.session.get({ sessionID: parentID }).catch(() => undefined)
    const parsed = parseModel(child.model)
    const model = parsed ? { providerID: parsed.provider, id: parsed.model } : parent?.model
    const agent = parent?.agent ?? this.bridge.agentOf(parentID)
    const session = await this.bridge.ctx.session.create({
      title: `${child.name} · recurse`,
      ...(agent ? { agent } : {}),
      ...(model ? { model } : {}),
      location: { directory: child.session_dir },
      metadata: { "recurse.parent": parentID, "recurse.child_id": child.child_id, "recurse.name": child.name },
    })

    this.created.set(child.child_id, session.id)
    return session.id
  }

  private async prompt(childID: string, sessionID: string, text: string): Promise<void> {
    await this.bridge.ctx.session.prompt({ sessionID, text })
    this.unprompted.delete(childID)
    this.created.delete(childID)
  }
}
