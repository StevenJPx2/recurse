import type { ErrorCode } from "./protocol.ts"

/** Daemon error codes plus the client-side failures that never reach the wire. */
export type RecurseErrorCode = ErrorCode | "unavailable" | "protocol_mismatch" | "invalid_response" | "timeout"

export class RecurseError extends Error {
  override readonly name = "RecurseError"

  constructor(
    readonly code: RecurseErrorCode,
    message: string,
    readonly status?: number,
  ) {
    super(message)
  }

  toJSON(): { code: RecurseErrorCode; message: string } {
    return { code: this.code, message: this.message }
  }
}

export function isRecurseError(error: unknown, code?: RecurseErrorCode): error is RecurseError {
  return error instanceof RecurseError && (code === undefined || error.code === code)
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
