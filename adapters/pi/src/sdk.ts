import type { AgentSession, CreateAgentSessionOptions, DefaultResourceLoader, ExtensionAPI, ExtensionContext, ResourceLoader } from "@earendil-works/pi-coding-agent"
import type { RecurseClient } from "@recurse/client"

export type ResourceLoaderOptions = ConstructorParameters<typeof DefaultResourceLoader>[0]

/** The SDK surface used for in-process children, loaded lazily so the extension stays cheap to load. */
export interface PiSdk {
  VERSION: string
  createAgentSession(options: CreateAgentSessionOptions): Promise<{ session: AgentSession }>
  SessionManager: {
    create?(cwd: string, sessionDir?: string, options?: { parentSession?: string }): NonNullable<CreateAgentSessionOptions["sessionManager"]>
    inMemory(cwd?: string): NonNullable<CreateAgentSessionOptions["sessionManager"]>
  }
  /** Present in real Pi; lets a child load recurse even when the parent loaded it with `-e`. */
  DefaultResourceLoader?: new (options: ResourceLoaderOptions) => ResourceLoader
  getAgentDir?(): string
}

export async function loadSdk(): Promise<PiSdk> {
  const sdk = await import("@earendil-works/pi-coding-agent")

  return {
    VERSION: sdk.VERSION,
    createAgentSession: sdk.createAgentSession,
    SessionManager: sdk.SessionManager,
    DefaultResourceLoader: sdk.DefaultResourceLoader,
    getAgentDir: sdk.getAgentDir,
  }
}

export interface PiOptions {
  connect?: () => Promise<RecurseClient>
  env?: NodeJS.ProcessEnv
  guidance?: string
  sdk?: () => Promise<PiSdk>
  /** The recurse extension factory itself, inlined into child sessions. */
  extension?: (pi: ExtensionAPI) => void
}

export type { AgentSession, ExtensionAPI, ExtensionContext }
