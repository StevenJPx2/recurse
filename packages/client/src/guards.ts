import {
  CELL_STATUSES,
  CHILD_STATUSES,
  ERROR_CODES,
  HOST_NAMES,
  KERNEL_STATES,
  TARGET_KINDS,
  type AgentMessageEvent,
  type BashFinishedEvent,
  type CellError,
  type CellResult,
  type ChildInfo,
  type ChildSpawnEvent,
  type HandleInfo,
  type Health,
  type HostInfo,
  type KernelExitedEvent,
  type KernelStatus,
  type RecurseEvent,
  type RegisterResult,
  type SkillInfo,
  type SseFrame,
  type Target,
  type WireError,
} from "./protocol.ts"

export type Guard<T> = (value: unknown) => value is T

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

export const isString = (value: unknown): value is string => typeof value === "string"
export const isBoolean = (value: unknown): value is boolean => typeof value === "boolean"
export const isInteger = (value: unknown): value is number => Number.isInteger(value)

export function oneOf<const T extends readonly string[]>(values: T): Guard<T[number]> {
  return (value): value is T[number] => typeof value === "string" && values.includes(value)
}

export function nullable<T>(guard: Guard<T>): Guard<T | null> {
  return (value): value is T | null => value === null || guard(value)
}

export function arrayOf<T>(guard: Guard<T>): Guard<T[]> {
  return (value): value is T[] => Array.isArray(value) && value.every(guard)
}

type Shape<T> = { [K in keyof T]-?: Guard<T[K]> }

/** Checks declared fields only; unknown response fields are ignored per the protocol. */
export function shape<T>(fields: Shape<T>, optional: readonly (keyof T)[] = []): Guard<T> {
  return (value): value is T => {
    if (!isRecord(value)) return false

    return Object.entries(fields).every(([key, guard]) => {
      if (!(key in value) && optional.includes(key as keyof T)) return true

      return (guard as Guard<unknown>)(value[key])
    })
  }
}

export const isTarget: Guard<Target> = (value): value is Target =>
  isRecord(value) && oneOf(TARGET_KINDS)(value.kind) && isString(value.id) && value.id.length >= 1 && value.id.length <= 256

export const isWireError = shape<WireError>({ code: oneOf(ERROR_CODES), message: isString })

export const isHostInfo = shape<HostInfo>(
  { name: oneOf(HOST_NAMES), version: isString, supports_children: isBoolean },
  ["version"],
)

export const isHealth = shape<Health>({
  ok: isBoolean,
  name: isString,
  version: isString,
  protocol: isInteger,
  pid: isInteger,
  started_at: isInteger,
})

export const isChildInfo = shape<ChildInfo>({
  child_id: isString,
  name: isString,
  task: isString,
  model: nullable(isString),
  status: oneOf(CHILD_STATUSES),
  parent: isTarget,
  session: nullable(isTarget),
  session_dir: isString,
  depth: isInteger,
  created_at: isInteger,
})

export const isRegisterResult = shape<RegisterResult>({
  target: isTarget,
  depth: isInteger,
  parent: nullable(isTarget),
  child: nullable(isChildInfo),
})

export const isCellError = shape<CellError>({ ename: isString, evalue: isString, traceback: isString })

export const isCellResult = shape<CellResult>({
  status: oneOf(CELL_STATUSES),
  stdout: isString,
  stderr: isString,
  result: nullable(isString),
  error: nullable(isCellError),
  execution_count: isInteger,
  duration_ms: isInteger,
  truncated: isBoolean,
})

export const isHandleInfo = shape<HandleInfo>({ handle_id: isString, pid: isInteger, command: isString, running: isBoolean })

export const isKernelStatus = shape<KernelStatus>({
  state: oneOf(KERNEL_STATES),
  pid: nullable(isInteger),
  execution_count: isInteger,
  started_at: nullable(isInteger),
  cwd: isString,
  handles: arrayOf(isHandleInfo),
})

export const isSkillInfo = shape<SkillInfo>({
  name: isString,
  description: isString,
  path: isString,
  python: nullable(isString),
  hidden: isBoolean,
})

export const isEvent = shape<RecurseEvent>({
  id: isString,
  target: isTarget,
  kind: isString,
  actionable: isBoolean,
  at: isInteger,
  text: isString,
  payload: (_value): _value is unknown => true,
})

const isSpawnPayload = shape<ChildSpawnEvent["payload"]>({ child: isChildInfo, prompt: isString })

const isMessagePayload = shape<AgentMessageEvent["payload"]>({
  from: shape<AgentMessageEvent["payload"]["from"]>({
    role: oneOf(["parent", "child"] as const),
    name: nullable(isString),
    child_id: nullable(isString),
  }),
  message: isString,
})

const isBashPayload = shape<BashFinishedEvent["payload"]>({
  handle_id: isString,
  pid: isInteger,
  exit_code: nullable(isInteger),
  command: isString,
})

const isExitPayload = shape<KernelExitedEvent["payload"]>({
  pid: isInteger,
  exit_code: nullable(isInteger),
  signal: nullable(isInteger),
})

function eventOf<E extends RecurseEvent>(kind: E["kind"], payload: Guard<E["payload"]>): Guard<E> {
  return (value): value is E => isEvent(value) && value.kind === kind && payload(value.payload)
}

export const isChildSpawnEvent = eventOf<ChildSpawnEvent>("child.spawn", isSpawnPayload)
export const isAgentMessageEvent = eventOf<AgentMessageEvent>("agent.message", isMessagePayload)
export const isBashFinishedEvent = eventOf<BashFinishedEvent>("bash.finished", isBashPayload)
export const isKernelExitedEvent = eventOf<KernelExitedEvent>("kernel.exited", isExitPayload)

export function isSseFrame(value: unknown): value is SseFrame {
  if (!isRecord(value)) return false
  if (value.type === "subscribed") return isTarget(value.target)
  if (value.type === "heartbeat") return isInteger(value.at)

  return value.type === "event" && isTarget(value.target) && arrayOf(isEvent)(value.events)
}
