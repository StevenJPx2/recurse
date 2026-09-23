import {
  BoundedSet,
  DaemonConnection,
  ensureDaemon,
  errorMessage,
  isChildSpawnEvent,
  type RecurseClient,
  type RecurseEvent,
  type RegisterResult,
  type SkillInfo,
  type Subscription,
  type Target,
} from "@recurse/client"
import { ChildSpawner } from "./children.ts"
import { TARGET_KIND, type AdapterOptions, type HostContext } from "./types.ts"

const SKILLS_TTL_MS = 60_000
const STORAGE_KEY = "sessions"

interface SessionState {
  model?: string
  agent?: string
  registeredModel?: string | undefined
  registered?: Promise<RegisterResult>
  subscription?: Promise<Subscription | undefined>
  skills?: { at: number; skills: SkillInfo[] }
}

export class RecurseBridge {
  private readonly connection: DaemonConnection
  private closed = false
  private readonly sessions = new Map<string, SessionState>()
  private readonly handled = new BoundedSet(2_048)
  private readonly children: ChildSpawner
  private readonly maxSessions: number

  constructor(
    readonly ctx: HostContext,
    private readonly options: AdapterOptions = {},
  ) {
    this.maxSessions = options.maxSessions ?? 256
    this.connection = new DaemonConnection(options.connect ?? (() => ensureDaemon(options.env ? { env: options.env } : {})))
    this.children = new ChildSpawner(this)
  }

  target(sessionID: string): Target {
    return { kind: TARGET_KIND, id: sessionID }
  }

  daemon(): Promise<RecurseClient> {
    return this.connection.get()
  }

  forget(error: unknown): void {
    this.connection.forget(error)
  }

  noteSession(sessionID: string, model?: string, agent?: string): void {
    const state = this.state(sessionID)

    if (agent) state.agent = agent
    if (!model) return

    state.model = model
    if (state.registered && state.registeredModel !== model) void this.register(sessionID, true).catch(() => {})
  }

  agentOf(sessionID: string): string | undefined {
    return this.sessions.get(sessionID)?.agent
  }

  register(sessionID: string, force = false): Promise<RegisterResult> {
    const state = this.state(sessionID)

    if (state.registered && !force) return state.registered

    const registered = this.registerNow(sessionID, state)

    state.registered = registered
    registered.catch(() => {
      if (state.registered === registered) delete state.registered
    })
    return registered
  }

  /** Register and subscribe a session; never rejects. */
  async track(sessionID: string): Promise<boolean> {
    if (this.closed) return false

    try {
      await this.register(sessionID)
    } catch (error) {
      this.forget(error)
      return false
    }

    const state = this.state(sessionID)

    state.subscription ??= this.subscribe(sessionID).then((subscription) => {
      if (!subscription) delete state.subscription
      return subscription
    })

    return (await state.subscription) !== undefined
  }

  async skills(sessionID: string): Promise<SkillInfo[]> {
    const state = this.state(sessionID)

    if (state.skills && Date.now() - state.skills.at < SKILLS_TTL_MS) return state.skills.skills
    const client = this.connection.current
    if (!client) return state.skills?.skills ?? []

    try {
      const params = state.registered ? { target: this.target(sessionID) } : {}
      const { skills } = await client.rpc("skills.list", params, { timeoutMs: 2_000 })

      state.skills = { at: Date.now(), skills }
      return skills
    } catch (error) {
      this.forget(error)
      return state.skills?.skills ?? []
    }
  }

  async restore(): Promise<void> {
    try {
      const saved = await this.ctx.storage.get(STORAGE_KEY)
      const ids = Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string") : []

      await Promise.all(ids.slice(-this.maxSessions).map((id) => this.track(id)))
    } catch (error) {
      console.error(`[recurse] could not restore sessions: ${errorMessage(error)}`)
    }
  }

  async handle(event: RecurseEvent): Promise<void> {
    if (this.handled.has(event.id)) return

    const sessionID = event.target.id

    if (event.kind === "child.spawn") {
      if (isChildSpawnEvent(event)) await this.children.spawn(event)
      else console.error(`[recurse] ignoring malformed child.spawn event ${event.id}`)
    } else if (event.actionable) {
      await this.ctx.session.prompt({ sessionID, text: event.text })
    } else {
      await this.ctx.session.synthetic({ sessionID, text: event.text })
    }

    this.handled.add(event.id)
  }

  async close(): Promise<void> {
    this.closed = true

    const subscriptions = [...this.sessions.values()].map((state) => state.subscription)

    this.sessions.clear()
    for (const pending of subscriptions) (await pending?.catch(() => undefined))?.close()
  }

  private async registerNow(sessionID: string, state: SessionState): Promise<RegisterResult> {
    const client = await this.daemon()
    const info = await this.ctx.session.get({ sessionID }).catch(() => undefined)
    const model = info?.model ? `${info.model.providerID}/${info.model.id}` : state.model

    if (info?.agent) state.agent = info.agent

    const result = await client.rpc("target.register", {
      target: this.target(sessionID),
      cwd: info?.location.directory ?? this.ctx.location.directory,
      host: { name: "opencode", version: this.ctx.app.version, supports_children: true },
      ...(model ? { model } : {}),
    })

    state.registeredModel = model
    return result
  }

  private async subscribe(sessionID: string): Promise<Subscription | undefined> {
    try {
      const client = await this.daemon()
      const subscription = await client.subscribe(this.target(sessionID), (event) => this.handle(event))

      if (this.closed || !this.sessions.has(sessionID)) {
        subscription.close()
        return undefined
      }

      void this.persist()
      return subscription
    } catch (error) {
      this.forget(error)
      console.error(`[recurse] could not subscribe ${sessionID}: ${errorMessage(error)}`)
      return undefined
    }
  }

  private state(sessionID: string): SessionState {
    const existing = this.sessions.get(sessionID)

    if (existing) {
      this.sessions.delete(sessionID)
      this.sessions.set(sessionID, existing)
      return existing
    }

    const created: SessionState = {}

    this.sessions.set(sessionID, created)
    this.evict()
    return created
  }

  private evict(): void {
    for (const [sessionID, state] of this.sessions) {
      if (this.sessions.size <= this.maxSessions) return

      this.sessions.delete(sessionID)
      void state.subscription?.then((subscription) => subscription?.close())
    }
  }

  private async persist(): Promise<void> {
    try {
      const tracked = [...this.sessions].filter(([, state]) => state.subscription).map(([sessionID]) => sessionID)

      await this.ctx.storage.set(STORAGE_KEY, tracked)
    } catch {
      // Restore on load is best-effort.
    }
  }
}
