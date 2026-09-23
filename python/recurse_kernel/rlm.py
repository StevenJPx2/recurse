"""`rlm` (children + host bridge) and `agent_message`."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from typing import Any

from . import errors
from .protocol import Channel


@dataclass(frozen=True)
class ChildInfo:
    child_id: str
    name: str
    task: str
    model: str | None
    status: str
    parent: dict[str, Any] | None
    session: dict[str, Any] | None
    session_dir: str
    depth: int
    created_at: int

    @classmethod
    def from_wire(cls, data: dict[str, Any]) -> ChildInfo:
        return cls(
            child_id=str(data.get("child_id", "")),
            name=str(data.get("name", "")),
            task=str(data.get("task", "")),
            model=data.get("model"),
            status=str(data.get("status", "")),
            parent=data.get("parent"),
            session=data.get("session"),
            session_dir=str(data.get("session_dir", "")),
            depth=int(data.get("depth", 0)),
            created_at=int(data.get("created_at", 0)),
        )

    @property
    def rlm_child_id(self) -> str:
        return self.child_id

    @property
    def session_name(self) -> str:
        return self.name

    @property
    def active_session_id(self) -> str | None:
        return self.session.get("id") if self.session else None

    def __repr__(self) -> str:
        return (
            f"ChildInfo(name={self.name!r}, status={self.status!r}, child_id={self.child_id!r}, "
            f"active_session_id={self.active_session_id!r}, model={self.model!r})"
        )


@dataclass(frozen=True)
class ChildHandle:
    """Admission handle returned by `rlm.spawn`; never carries the child's answer."""

    rlm_child_id: str
    name: str
    session_dir: str
    model: str | None
    status: str = "pending"


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, default))
    except ValueError:
        return default


def _env_target() -> dict[str, Any] | None:
    try:
        return json.loads(os.environ["RECURSE_TARGET"])
    except (KeyError, ValueError):
        return None


class Rlm:
    RecurseError = errors.RecurseError
    UnsupportedHost = errors.UnsupportedHost
    DepthExceeded = errors.DepthExceeded
    LimitExceeded = errors.LimitExceeded
    NotFound = errors.NotFound
    InvalidRequest = errors.InvalidRequest

    def __init__(self, channel: Channel) -> None:
        self._channel = channel
        self.depth = _env_int("RECURSE_DEPTH", 0)
        self.max_depth = _env_int("RECURSE_MAX_DEPTH", 1)
        self.target = _env_target()

    async def host_request(self, method: str, /, **params: Any) -> Any:
        """Send a typed request to the daemon and return its raw result."""
        return await self._channel.request(method, params)

    async def spawn(self, task: str, *, name: str, model: str | None = None) -> ChildHandle:
        """Admit a child agent. Returns at admission; results arrive via agent_message or files."""
        params: dict[str, Any] = {"task": task, "name": name}
        if model is not None:
            params["model"] = model

        info = ChildInfo.from_wire(await self.host_request("rlm.spawn", **params))
        return ChildHandle(info.child_id, info.name, info.session_dir, info.model, info.status)

    async def list_subagents(self) -> list[ChildInfo]:
        result = await self.host_request("rlm.list_subagents")
        return [ChildInfo.from_wire(child) for child in (result or {}).get("children", [])]

    async def delete_subagent(self, child: ChildInfo | ChildHandle | str) -> ChildInfo:
        """Delete by ChildInfo, ChildHandle, child id (`c_…`), or name."""
        if isinstance(child, (ChildInfo, ChildHandle)):
            params = {"child_id": child.rlm_child_id}
        elif isinstance(child, str) and "_" in child:
            params = {"child_id": child}
        elif isinstance(child, str):
            params = {"name": child}
        else:
            raise TypeError("delete_subagent expects a ChildInfo, ChildHandle, child id, or name")

        result = await self.host_request("rlm.delete_subagent", **params)
        return ChildInfo.from_wire((result or {}).get("child", {}))

    def __repr__(self) -> str:
        return f"<rlm depth={self.depth} max_depth={self.max_depth} target={self.target}>"


class AgentMessage:
    def __init__(self, channel: Channel) -> None:
        self._channel = channel

    async def send(
        self,
        message: str,
        receiver_role: str = "parent",
        receiver_name: str | None = None,
    ) -> str:
        """Queue a message to the parent or a named child. Returns the event id."""
        if receiver_role not in ("parent", "child"):
            raise errors.InvalidRequest("receiver_role must be 'parent' or 'child'")
        if receiver_role == "child" and not receiver_name:
            raise errors.InvalidRequest("receiver_name is required when receiver_role='child'")

        params: dict[str, Any] = {"message": str(message), "receiver_role": receiver_role}
        if receiver_name is not None:
            params["receiver_name"] = receiver_name

        result = await self._channel.request("agent_message.send", params)
        return str((result or {}).get("event_id", ""))

    def __repr__(self) -> str:
        return "<agent_message: await agent_message.send(message, receiver_role='parent'|'child', receiver_name=None)>"
