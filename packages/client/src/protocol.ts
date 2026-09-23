// Wire types for docs/protocol.md (version 1). Field names are snake_case on the wire.

export const PROTOCOL_VERSION = 1

export const TARGET_KINDS = ["opencode-session", "pi-session", "mcp", "cli"] as const
export type TargetKind = (typeof TARGET_KINDS)[number]

export interface Target {
  kind: TargetKind
  id: string
}

export const HOST_NAMES = ["opencode", "pi", "mcp", "cli"] as const
export type HostName = (typeof HOST_NAMES)[number]

export interface HostInfo {
  name: HostName
  version?: string
  supports_children: boolean
}

export const ERROR_CODES = [
  "invalid_request",
  "unauthorized",
  "not_found",
  "busy",
  "kernel_unavailable",
  "unsupported_host",
  "depth_exceeded",
  "limit_exceeded",
  "internal",
] as const
export type ErrorCode = (typeof ERROR_CODES)[number]

export interface WireError {
  code: ErrorCode
  message: string
}

export interface RpcRequest {
  id?: number
  method: string
  params?: unknown
}

export type RpcResponse<R = unknown> =
  | { id?: number; result: R; error?: undefined }
  | { id?: number; error: WireError; result?: undefined }

export interface Health {
  ok: boolean
  name: string
  version: string
  protocol: number
  pid: number
  started_at: number
}

export const CHILD_STATUSES = ["pending", "running", "failed", "deleted"] as const
export type ChildStatus = (typeof CHILD_STATUSES)[number]

export interface ChildInfo {
  child_id: string
  name: string
  task: string
  model: string | null
  status: ChildStatus
  parent: Target
  session: Target | null
  session_dir: string
  depth: number
  created_at: number
}

export interface RegisterParams {
  target: Target
  cwd: string
  host: HostInfo
  model?: string
}

export interface RegisterResult {
  target: Target
  depth: number
  parent: Target | null
  child: ChildInfo | null
}

export interface ExecuteParams {
  target: Target
  code: string
  timeout_sec?: number
}

export const CELL_STATUSES = ["ok", "error", "timeout", "interrupted"] as const
export type CellStatus = (typeof CELL_STATUSES)[number]

export interface CellError {
  ename: string
  evalue: string
  traceback: string
}

export interface CellResult {
  status: CellStatus
  stdout: string
  stderr: string
  result: string | null
  error: CellError | null
  execution_count: number
  duration_ms: number
  truncated: boolean
}

export interface HandleInfo {
  handle_id: string
  pid: number
  command: string
  running: boolean
}

export const KERNEL_STATES = ["absent", "starting", "idle", "busy", "dead"] as const
export type KernelState = (typeof KERNEL_STATES)[number]

export interface KernelStatus {
  state: KernelState
  pid: number | null
  execution_count: number
  started_at: number | null
  cwd: string
  handles: HandleInfo[]
}

export interface SkillInfo {
  name: string
  description: string
  path: string
  python: string | null
  hidden: boolean
}

export interface TargetParams {
  target: Target
}

export interface ChildParams extends TargetParams {
  child_id: string
}

export interface RpcMethods {
  health: { params: Record<string, never>; result: Health }
  "target.register": { params: RegisterParams; result: RegisterResult }
  "kernel.execute": { params: ExecuteParams; result: CellResult }
  "kernel.interrupt": { params: TargetParams; result: { interrupted: boolean } }
  "kernel.restart": { params: TargetParams; result: { restarted: boolean } }
  "kernel.status": { params: TargetParams; result: KernelStatus }
  "children.list": { params: TargetParams; result: { children: ChildInfo[] } }
  "children.bind": { params: ChildParams & { session: Target }; result: { child: ChildInfo } }
  "children.fail": { params: ChildParams & { reason: string }; result: { child: ChildInfo } }
  "children.delete": { params: ChildParams; result: { child: ChildInfo } }
  "events.ack": { params: TargetParams & { event_ids: string[] }; result: { acked: number } }
  "events.list": { params: TargetParams; result: { events: RecurseEvent[] } }
  "skills.list": { params: Partial<TargetParams>; result: { skills: SkillInfo[] } }
}

export type RpcMethod = keyof RpcMethods
export type RpcParams<M extends RpcMethod> = RpcMethods[M]["params"]
export type RpcResult<M extends RpcMethod> = RpcMethods[M]["result"]

export const EVENT_KINDS = ["child.spawn", "agent.message", "bash.finished", "kernel.exited"] as const
export type KnownEventKind = (typeof EVENT_KINDS)[number]

/** Every event shares this envelope; unknown kinds are delivered with an opaque payload. */
export interface RecurseEvent {
  id: string
  target: Target
  kind: string
  actionable: boolean
  at: number
  text: string
  payload: unknown
}

export interface ChildSpawnEvent extends RecurseEvent {
  kind: "child.spawn"
  payload: { child: ChildInfo; prompt: string }
}

export interface MessageSender {
  role: "parent" | "child"
  name: string | null
  child_id: string | null
}

export interface AgentMessageEvent extends RecurseEvent {
  kind: "agent.message"
  payload: { from: MessageSender; message: string }
}

export interface BashFinishedEvent extends RecurseEvent {
  kind: "bash.finished"
  payload: { handle_id: string; pid: number; exit_code: number | null; command: string }
}

export interface KernelExitedEvent extends RecurseEvent {
  kind: "kernel.exited"
  payload: { pid: number; exit_code: number | null; signal: number | null }
}

export type KnownEvent = ChildSpawnEvent | AgentMessageEvent | BashFinishedEvent | KernelExitedEvent

export type SseFrame =
  | { type: "subscribed"; target: Target }
  | { type: "event"; target: Target; events: RecurseEvent[] }
  | { type: "heartbeat"; at: number }
