import type { HostContext } from "../../src/types.ts"

type Callback = (event: any) => Promise<void> | void

export interface FakeTool {
  name: string
  description: string
  input: unknown
  options?: { codemode?: boolean }
  execute(input: unknown, context: unknown): Promise<{ content: string }>
}

export interface FakeSession {
  id: string
  agent?: string
  model?: { providerID: string; id: string }
  location: { directory: string }
}

/** Records everything the adapter asks of OpenCode. */
export class FakeHost {
  readonly tools: FakeTool[] = []
  readonly hooks = new Map<string, Callback>()
  readonly prompts: { sessionID: string; text: string }[] = []
  readonly synthetics: { sessionID: string; text: string }[] = []
  readonly creates: Record<string, unknown>[] = []
  readonly sessions = new Map<string, FakeSession>()
  readonly storage = new Map<string, unknown>()
  createError: Error | undefined
  private nextSession = 1

  constructor() {
    this.sessions.set("ses_abc", {
      id: "ses_abc",
      agent: "build",
      model: { providerID: "anthropic", id: "claude-sonnet-5" },
      location: { directory: "/abs/project" },
    })
  }

  context(): HostContext {
    const host = this
    const registration = { dispose: async () => {} }

    return {
      app: { name: "opencode", version: "2.0.15", channel: "latest" },
      location: { directory: "/plugin/location" },
      tool: {
        transform: async (callback: (editor: { add(tool: FakeTool): void }) => void) => {
          callback({ add: (tool) => host.tools.push(tool) })
          return registration
        },
      },
      session: {
        hook: async (name: string, callback: Callback) => {
          host.hooks.set(name, callback)
          return registration
        },
        get: async ({ sessionID }: { sessionID: string }) => {
          const session = host.sessions.get(sessionID)
          if (!session) throw new Error(`no session ${sessionID}`)
          return session
        },
        create: async (input: Record<string, unknown>) => {
          host.creates.push(input)
          if (host.createError) throw host.createError

          const id = `ses_child_${host.nextSession++}`
          const location = input.location as { directory: string }
          host.sessions.set(id, { id, location })
          return { id }
        },
        prompt: async (input: { sessionID: string; text: string }) => {
          host.prompts.push(input)
          return {}
        },
        synthetic: async (input: { sessionID: string; text: string }) => {
          host.synthetics.push(input)
          return {}
        },
      },
      storage: {
        get: async (key: string) => host.storage.get(key),
        set: async (key: string, value: unknown) => void host.storage.set(key, value),
      },
    } as unknown as HostContext
  }

  hook(name: string): Callback {
    const callback = this.hooks.get(name)
    if (!callback) throw new Error(`hook ${name} not registered`)
    return callback
  }

  tool(name: string): FakeTool {
    const tool = this.tools.find((candidate) => candidate.name === name)
    if (!tool) throw new Error(`tool ${name} not registered`)
    return tool
  }
}

export function contextEvent(sessionID: string, tools: string[], system: { type: "text"; text: string }[] = []) {
  return {
    sessionID,
    agent: "build",
    model: { providerID: "anthropic", id: "claude-sonnet-5" },
    system,
    messages: [],
    options: {},
    tools: Object.fromEntries(tools.map((name) => [name, { description: name, input: {} }])),
  }
}

export function toolContext(sessionID: string, signal = new AbortController().signal) {
  return { sessionID, agent: "build", messageID: "msg_1", id: "call_1", signal, progress: async () => {} }
}
