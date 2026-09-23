import {
  BoundedSet,
  DaemonConnection,
  ensureDaemon,
  errorMessage,
  formatCell,
  formatFailure,
  guidanceBody,
  isChildSpawnEvent,
  isRecurseError,
  loadGuidance,
  type RecurseEvent,
  type RegisterResult,
  type SkillInfo,
  type Subscription,
  type Target,
} from "@recurse/client"
import { PiChildren } from "./children.ts"
import { loadSdk, type ExtensionAPI, type ExtensionContext, type PiOptions, type PiSdk } from "./sdk.ts"

const SKILLS_TTL_MS = 60_000

export interface CellInput {
  code: string
  timeout_sec?: number
}

/** One instance per extension runtime; Pi runs one session per runtime (children get their own). */
export class PiRecurse {
  readonly connection: DaemonConnection
  readonly guidance: string
  ctx: ExtensionContext | undefined
  private readonly children: PiChildren
  private readonly handled = new BoundedSet(2_048)
  private readonly registered = new Map<string, { model: string | undefined; promise: Promise<RegisterResult> }>()
  private subscription: { key: string; promise: Promise<Subscription | undefined> } | undefined
  private skillsCache: { at: number; skills: SkillInfo[] } | undefined
  private sdkPromise: Promise<PiSdk> | undefined

  constructor(
    readonly pi: ExtensionAPI,
    private readonly options: PiOptions = {},
  ) {
    this.connection = new DaemonConnection(options.connect ?? (() => ensureDaemon(options.env ? { env: options.env } : {})))
    this.guidance = options.guidance ?? loadGuidance(import.meta.url)
    this.children = new PiChildren(this)
  }

  get extension(): PiOptions["extension"] {
    return this.options.extension
  }

  sdk(): Promise<PiSdk> {
    this.sdkPromise ??= (this.options.sdk ?? loadSdk)()
    return this.sdkPromise
  }

  target(ctx: ExtensionContext): Target {
    return { kind: "pi-session", id: ctx.sessionManager.getSessionId() }
  }

  /** session_start: register and subscribe; never rejects. */
  async start(ctx: ExtensionContext): Promise<void> {
    this.ctx = ctx

    const target = this.target(ctx)

    if (this.subscription?.key === target.id) return

    const previous = this.subscription

    this.subscription = { key: target.id, promise: this.subscribe(ctx, target) }
    void previous?.promise.then((subscription) => subscription?.close())
    await this.subscription.promise
  }

  register(ctx: ExtensionContext, force = false): Promise<RegisterResult> {
    const target = this.target(ctx)
    const model = ctx.model ? `${ctx.model.provider}/${ctx.model.id}` : undefined
    const existing = this.registered.get(target.id)

    if (existing && !force && existing.model === model) return existing.promise

    const promise = this.registerNow(ctx, target, model)

    this.registered.set(target.id, { model, promise })
    promise.catch(() => {
      if (this.registered.get(target.id)?.promise === promise) this.registered.delete(target.id)
    })
    return promise
  }

  async execute(ctx: ExtensionContext, input: CellInput, signal?: AbortSignal): Promise<string> {
    try {
      const client = await this.connection.get()
      const target = this.target(ctx)
      const params = { target, code: input.code, ...(input.timeout_sec ? { timeout_sec: Math.min(Math.ceil(input.timeout_sec), 3_600) } : {}) }
      const interrupt = (): void => void client.rpc("kernel.interrupt", { target }).catch(() => {})

      await this.register(ctx)
      void this.start(ctx)
      signal?.addEventListener("abort", interrupt, { once: true })

      try {
        return formatCell(await client.rpc("kernel.execute", params))
      } catch (error) {
        if (!isRecurseError(error, "not_found")) throw error

        await this.register(ctx, true)
        return formatCell(await client.rpc("kernel.execute", params))
      } finally {
        signal?.removeEventListener("abort", interrupt)
      }
    } catch (error) {
      this.connection.forget(error)
      return formatFailure(error)
    }
  }

  /** Guidance plus the skill list, for the `recurse-guidance` system prompt section. */
  async guidanceText(ctx: ExtensionContext | undefined): Promise<string> {
    return guidanceBody(this.guidance, await this.skills(ctx))
  }

  async handle(event: RecurseEvent): Promise<void> {
    if (this.handled.has(event.id)) return

    if (event.kind === "child.spawn") {
      if (isChildSpawnEvent(event)) await this.children.spawn(event)
      else console.error(`[recurse] ignoring malformed child.spawn event ${event.id}`)
    } else {
      this.pi.sendMessage(
        { customType: "recurse", content: event.text, display: true, details: event },
        { triggerTurn: event.actionable, deliverAs: "steer" },
      )
    }

    this.handled.add(event.id)
  }

  async shutdown(): Promise<void> {
    const subscription = this.subscription

    this.subscription = undefined
    this.ctx = undefined
    ;(await subscription?.promise)?.close()
    await this.children.dispose()
  }

  private async registerNow(ctx: ExtensionContext, target: Target, model: string | undefined): Promise<RegisterResult> {
    const client = await this.connection.get()
    const version = await this.sdk().then((sdk) => sdk.VERSION).catch(() => undefined)

    return client.rpc("target.register", {
      target,
      cwd: ctx.cwd,
      host: { name: "pi", ...(version ? { version } : {}), supports_children: true },
      ...(model ? { model } : {}),
    })
  }

  private async subscribe(ctx: ExtensionContext, target: Target): Promise<Subscription | undefined> {
    try {
      await this.register(ctx)
      return await (await this.connection.get()).subscribe(target, (event) => this.handle(event))
    } catch (error) {
      this.connection.forget(error)
      console.error(`[recurse] could not subscribe ${target.id}: ${errorMessage(error)}`)
      if (this.subscription?.key === target.id) this.subscription = undefined
      return undefined
    }
  }

  private async skills(ctx: ExtensionContext | undefined): Promise<SkillInfo[]> {
    if (this.skillsCache && Date.now() - this.skillsCache.at < SKILLS_TTL_MS) return this.skillsCache.skills

    const client = this.connection.current
    if (!client) return this.skillsCache?.skills ?? []

    try {
      const params = ctx && this.registered.has(this.target(ctx).id) ? { target: this.target(ctx) } : {}
      const { skills } = await client.rpc("skills.list", params, { timeoutMs: 2_000 })

      this.skillsCache = { at: Date.now(), skills }
      return skills
    } catch (error) {
      this.connection.forget(error)
      return this.skillsCache?.skills ?? []
    }
  }
}
