import type { Plugin } from "@opencode/plugin"
import type { RecurseClient } from "@recurse/client"

/** The slice of the V2 plugin context recurse uses; tests provide a fake with this shape. */
export type HostContext = Pick<Plugin.Context, "app" | "location" | "session" | "tool" | "storage">

export interface AdapterOptions {
  connect?: () => Promise<RecurseClient>
  env?: NodeJS.ProcessEnv
  maxSessions?: number
  guidance?: string
}

export const TARGET_KIND = "opencode-session"
