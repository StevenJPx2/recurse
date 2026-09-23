// Daemon ↔ kernel messages (docs/protocol.md §4). The TypeScript side never
// speaks this protocol; the types exist so fixtures are checked in every language.

import type { CellResult, ChildInfo, HandleInfo, SkillInfo, WireError } from "./protocol.ts"

export type ToKernelMessage =
  | { type: "execute"; id: string; code: string; timeout_ms: number }
  | { type: "interrupt" }
  | { type: "host_response"; id: string; result: unknown }
  | { type: "host_response"; id: string; error: WireError }
  | { type: "shutdown" }

export interface HostRequests {
  "rlm.spawn": { params: { task: string; name: string; model?: string | null }; result: ChildInfo }
  "rlm.list_subagents": { params: Record<string, never>; result: { children: ChildInfo[] } }
  "rlm.delete_subagent": { params: { child_id: string } | { name: string }; result: { child: ChildInfo } }
  "agent_message.send": {
    params: { message: string; receiver_role: "parent" | "child"; receiver_name?: string | null }
    result: { event_id: string }
  }
  "notice.bash_finished": {
    params: { handle_id: string; pid: number; exit_code: number | null; command: string }
    result: { event_id: string }
  }
  "notice.withdraw": { params: { event_id: string }; result: { withdrawn: boolean } }
  "skills.list": { params: Record<string, never>; result: { skills: SkillInfo[] } }
}

export type HostRequestMethod = keyof HostRequests

export const HOST_REQUEST_METHODS: readonly HostRequestMethod[] = [
  "rlm.spawn",
  "rlm.list_subagents",
  "rlm.delete_subagent",
  "agent_message.send",
  "notice.bash_finished",
  "notice.withdraw",
  "skills.list",
]

export type LogLevel = "debug" | "info" | "warn" | "error"

export type FromKernelMessage =
  | { type: "ready"; pid: number; python: string; protocol: number }
  | { type: "execute_result"; id: string; result: CellResult }
  | { type: "host_request"; id: string; method: HostRequestMethod; params: unknown }
  | { type: "handles"; handles: HandleInfo[] }
  | { type: "log"; level: LogLevel; message: string }
