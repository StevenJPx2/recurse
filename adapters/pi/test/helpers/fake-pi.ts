import type { ExtensionAPI, ExtensionContext, PiSdk } from "../../src/sdk.ts"

type Handler = (event: any, ctx: ExtensionContext) => unknown

export interface FakeToolDefinition {
  name: string
  parameters: unknown
  executionMode?: string
  execute(id: string, params: unknown, signal: AbortSignal | undefined, onUpdate: undefined, ctx: ExtensionContext): Promise<{ content: { type: string; text: string }[] }>
}

/** Records every ExtensionAPI call recurse makes. */
export class FakePi {
  readonly tools: FakeToolDefinition[] = []
  readonly handlers = new Map<string, Handler>()
  readonly messages: { message: Record<string, unknown>; options: Record<string, unknown> }[] = []
  activeTools: string[] | undefined

  api(): ExtensionAPI {
    return {
      registerTool: (tool: FakeToolDefinition) => void this.tools.push(tool),
      on: (name: string, handler: Handler) => {
        this.handlers.set(name, handler)
        return () => this.handlers.delete(name)
      },
      setActiveTools: (names: string[]) => {
        this.activeTools = names
      },
      sendMessage: (message: Record<string, unknown>, options: Record<string, unknown>) => void this.messages.push({ message, options }),
    } as unknown as ExtensionAPI
  }

  async emit(name: string, event: Record<string, unknown>, ctx: ExtensionContext): Promise<unknown> {
    const handler = this.handlers.get(name)
    if (!handler) throw new Error(`no handler for ${name}`)
    return handler({ type: name, ...event }, ctx)
  }
}

export function fakeContext(sessionId = "pi_parent"): ExtensionContext {
  return {
    cwd: "/abs/project",
    model: { provider: "anthropic", id: "claude-sonnet-5" },
    modelRegistry: { find: (provider: string, id: string) => ({ provider, id, resolved: true }) },
    sessionManager: { getSessionId: () => sessionId, getSessionFile: () => `/sessions/${sessionId}.jsonl` },
  } as unknown as ExtensionContext
}

export class FakeSdk {
  readonly created: Record<string, unknown>[] = []
  readonly managers: unknown[][] = []
  readonly prompts: string[] = []
  disposed = 0
  createError: Error | undefined
  /** When set, the fake SDK offers DefaultResourceLoader and records its options. */
  loaders: Record<string, any>[] | undefined

  sdk(): PiSdk {
    const fake = this

    return {
      VERSION: "0.87.1",
      SessionManager: {
        create: (...args: unknown[]) => {
          fake.managers.push(args)
          return { persisted: true } as never
        },
        inMemory: () => ({ persisted: false }) as never,
      },
      ...(fake.loaders
        ? {
            getAgentDir: () => "/agent",
            DefaultResourceLoader: class {
              reloaded = false
              constructor(readonly options: Record<string, any>) {
                fake.loaders?.push(options)
              }
              async reload() {
                this.reloaded = true
              }
            } as never,
          }
        : {}),
      createAgentSession: async (options) => {
        fake.created.push(options as Record<string, unknown>)
        if (fake.createError) throw fake.createError

        const session = {
          sessionManager: { getSessionId: () => `pi_child_${fake.created.length}` },
          prompt: async (text: string) => void fake.prompts.push(text),
          dispose: () => void fake.disposed++,
        }
        return { session: session as never }
      },
    }
  }
}
