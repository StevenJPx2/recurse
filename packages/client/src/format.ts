import { isRecurseError, errorMessage } from "./errors.ts"
import type { CellResult } from "./protocol.ts"

const STATUS_NOTES: Record<CellResult["status"], string> = {
  ok: "ok",
  error: "error",
  timeout: "timeout (the cell was interrupted; kernel state is kept)",
  interrupted: "interrupted",
}

/** Model-facing text for one executed cell. */
export function formatCell(cell: CellResult): string {
  const lines = [`[${STATUS_NOTES[cell.status]}] cell ${cell.execution_count} · ${cell.duration_ms} ms`]

  if (cell.stdout) lines.push("", "stdout:", trimEnd(cell.stdout))
  if (cell.stderr) lines.push("", "stderr:", trimEnd(cell.stderr))
  if (cell.result !== null) lines.push("", `Out: ${cell.result}`)
  if (cell.error) {
    lines.push("", `${cell.error.ename}: ${cell.error.evalue}`)
    if (cell.error.traceback) lines.push(trimEnd(cell.error.traceback))
  }
  if (cell.truncated) lines.push("", "[output truncated: long streams keep their tail; write large results to files and read slices]")

  return lines.join("\n")
}

/** Model-facing text for an RPC failure; tools return this instead of throwing into the host. */
export function formatFailure(error: unknown): string {
  if (isRecurseError(error, "busy")) {
    return "[busy] Another cell is still running in this session's kernel. End your turn and wait for its result or notice instead of retrying in a loop."
  }
  if (isRecurseError(error, "unavailable") || isRecurseError(error, "timeout")) {
    return `[recurse unavailable] ${errorMessage(error)}. The recurse daemon could not be reached; check that \`recurse daemon\` is installed and running.`
  }
  if (isRecurseError(error, "kernel_unavailable")) {
    return `[kernel unavailable] ${errorMessage(error)}. The Python kernel failed to start or died; the next call starts a fresh kernel.`
  }
  if (isRecurseError(error)) return `[${error.code}] ${error.message}`

  return `[error] ${errorMessage(error)}`
}

function trimEnd(text: string): string {
  return text.replace(/\s+$/, "")
}
