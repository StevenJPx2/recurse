"""Live handle registry: `handles` updates and background-finish notices."""

from __future__ import annotations

import asyncio
import os
import signal
import subprocess
import time
from collections.abc import Coroutine, Mapping
from typing import Any

from .bash import BashHandle, group_alive, signal_group
from .protocol import Channel
from .state import STATE

_CLOSE_GRACE_SEC = 1.0


class HandleRegistry:
    def __init__(self, channel: Channel) -> None:
        self._channel = channel
        self._live: dict[str, BashHandle] = {}
        self._deferred: list[BashHandle] = []
        self._tasks: set[asyncio.Task[Any]] = set()
        self._seq = 0
        self._closing = False

    def bash(
        self,
        command: str,
        *,
        cwd: str | os.PathLike[str] | None = None,
        env: Mapping[str, Any] | None = None,
        timeout: float | None = None,
    ) -> BashHandle:
        """Start `command` in its own process group and return a live handle."""
        full_env = dict(os.environ)
        full_env.update({str(key): str(value) for key, value in (env or {}).items()})

        proc = subprocess.Popen(
            command,
            shell=True,
            cwd=os.fspath(cwd) if cwd is not None else os.getcwd(),
            env=full_env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )

        self._seq += 1
        handle = BashHandle(self, f"h{self._seq}", command, proc, timeout)
        self._live[handle.handle_id] = handle
        self.changed()
        return handle

    def changed(self) -> None:
        if not self._closing:
            handles = [handle.info() for handle in self._live.values()]
            self._channel.send({"type": "handles", "handles": handles})

    def finished(self, handle: BashHandle) -> None:
        self._live.pop(handle.handle_id, None)
        if self._closing:
            return

        self.changed()
        if STATE.running is not None:
            self._deferred.append(handle)
        else:
            self._maybe_notify(handle)

    def cell_finished(self) -> None:
        deferred, self._deferred = self._deferred, []
        for handle in deferred:
            self._maybe_notify(handle)

    def _maybe_notify(self, handle: BashHandle) -> None:
        if handle.read_in_cell or handle.killed or handle.notice_state != "none":
            return
        handle.notice_state = "sending"
        self._spawn(self._send_notice(handle))

    async def _send_notice(self, handle: BashHandle) -> None:
        params = {
            "handle_id": handle.handle_id,
            "pid": handle.pid,
            "exit_code": handle.exit_code if handle.exit_code is not None else -1,
            "command": handle.command,
        }
        try:
            result = await self._channel.request("notice.bash_finished", params)
        except Exception as exc:
            handle.notice_state = "failed"
            self._channel.log("warn", f"notice.bash_finished for {handle.handle_id} failed: {exc}")
            return

        handle.notice_event_id = (result or {}).get("event_id")
        handle.notice_state = "sent"
        if handle.read_in_cell:
            await self._withdraw(handle)

    def withdraw(self, handle: BashHandle) -> None:
        """Called on the first in-cell read; a notice still in flight is withdrawn once sent."""
        if handle.notice_state == "sent":
            self._spawn(self._withdraw(handle))

    async def _withdraw(self, handle: BashHandle) -> None:
        if handle.notice_event_id is None:
            return
        try:
            await self._channel.request("notice.withdraw", {"event_id": handle.notice_event_id})
        except Exception as exc:
            self._channel.log("warn", f"notice.withdraw for {handle.handle_id} failed: {exc}")

    def _spawn(self, coro: Coroutine[Any, Any, None]) -> None:
        task = asyncio.get_running_loop().create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    def close(self) -> None:
        """Kill every live process group (kernel shutdown)."""
        self._closing = True
        live = list(self._live.values())

        # Repeat: a shell mid-fork can miss the first group signal.
        deadline = time.monotonic() + _CLOSE_GRACE_SEC
        while live and time.monotonic() < deadline:
            for handle in live:
                signal_group(handle.pid, signal.SIGKILL)
                handle.reap()
            live = [handle for handle in live if group_alive(handle.pid)]
            time.sleep(0.02)
