import { arrayOf, isCellResult, isHandleInfo, isInteger, isRecord, isString, isWireError, oneOf } from "./guards.ts"
import { HOST_REQUEST_METHODS, type FromKernelMessage, type ToKernelMessage } from "./kernel.ts"

const isLogLevel = oneOf(["debug", "info", "warn", "error"] as const)

export function isToKernelMessage(value: unknown): value is ToKernelMessage {
  if (!isRecord(value)) return false

  switch (value.type) {
    case "execute":
      return isString(value.id) && isString(value.code) && isInteger(value.timeout_ms)
    case "interrupt":
    case "shutdown":
      return true
    case "host_response":
      return isString(value.id) && ("error" in value ? isWireError(value.error) : "result" in value)
    default:
      return false
  }
}

export function isFromKernelMessage(value: unknown): value is FromKernelMessage {
  if (!isRecord(value)) return false

  switch (value.type) {
    case "ready":
      return isInteger(value.pid) && isString(value.python) && isInteger(value.protocol)
    case "execute_result":
      return isString(value.id) && isCellResult(value.result)
    case "host_request":
      return isString(value.id) && oneOf(HOST_REQUEST_METHODS)(value.method) && isRecord(value.params)
    case "handles":
      return arrayOf(isHandleInfo)(value.handles)
    case "log":
      return isLogLevel(value.level) && isString(value.message)
    default:
      return false
  }
}
