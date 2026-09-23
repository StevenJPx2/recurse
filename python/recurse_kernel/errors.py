"""Typed errors for daemon `host_response` failures."""

from __future__ import annotations

from typing import Any


class RecurseError(Exception):
    """A host request failed. `code` is the protocol error code."""

    code = "internal"

    def __init__(self, message: str, code: str | None = None) -> None:
        super().__init__(message)
        self.message = message
        if code is not None:
            self.code = code


class UnsupportedHost(RecurseError):
    code = "unsupported_host"


class DepthExceeded(RecurseError):
    code = "depth_exceeded"


class LimitExceeded(RecurseError):
    code = "limit_exceeded"


class NotFound(RecurseError):
    code = "not_found"


class InvalidRequest(RecurseError):
    code = "invalid_request"


_BY_CODE: dict[str, type[RecurseError]] = {
    cls.code: cls
    for cls in (UnsupportedHost, DepthExceeded, LimitExceeded, NotFound, InvalidRequest)
}


def from_wire(error: Any) -> RecurseError:
    if not isinstance(error, dict):
        return RecurseError(str(error))

    code = str(error.get("code") or "internal")
    message = str(error.get("message") or code)
    return _BY_CODE.get(code, RecurseError)(message, code)
