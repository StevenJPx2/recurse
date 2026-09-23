"""`bash()` handles: one process group per command, tracked until reaped."""

from __future__ import annotations

import asyncio
import os
import signal
import subprocess
from dataclasses import dataclass
from typing import IO, TYPE_CHECKING, Any

from .bounds import BASH_LIMIT, TailBuffer
from .state import in_live_cell

if TYPE_CHECKING:
    from .handles import HandleRegistry

_FOREGROUND_POLL_SEC = 0.02
_GROUP_POLL_SEC = 0.05
_KILL_GRACE_SEC = 2.0


@dataclass(frozen=True)
class BashResult:
    output: str
    exit_code: int
    stdout: str
    stderr: str

    def __repr__(self) -> str:
        return f"BashResult(exit_code={self.exit_code}, output={self.output!r})"


def group_alive(pgid: int) -> bool:
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def signal_group(pgid: int, sig: int) -> None:
    try:
        os.killpg(pgid, sig)
    except (ProcessLookupError, PermissionError):
        pass


class BashHandle:
    """A running shell command. Await it for a `BashResult`."""

    def __init__(
        self,
        registry: HandleRegistry,
        handle_id: str,
        command: str,
        proc: subprocess.Popen[bytes],
        timeout: float | None,
    ) -> None:
        self.handle_id = handle_id
        self.command = command
        self.pid = proc.pid
        self._registry = registry
        self._proc = proc
        self._loop = asyncio.get_running_loop()

        self._stdout = TailBuffer(BASH_LIMIT)
        self._stderr = TailBuffer(BASH_LIMIT)
        self._merged = TailBuffer(BASH_LIMIT)
        self._streams: list[IO[bytes]] = []
        self._attach(proc.stdout, self._stdout)
        self._attach(proc.stderr, self._stderr)

        self._exit_code: int | None = None
        self._foreground_done = asyncio.Event()
        self._group_done = asyncio.Event()
        self.read_in_cell = False
        self.killed = False
        self.notice_state = "none"
        self.notice_event_id: str | None = None

        self._timer = self._loop.call_later(timeout, self.kill) if timeout else None
        self._watcher = self._loop.create_task(self._watch())

    # -- output plumbing -------------------------------------------------

    def _attach(self, stream: IO[bytes] | None, buffer: TailBuffer) -> None:
        if stream is None:
            return
        os.set_blocking(stream.fileno(), False)
        self._loop.add_reader(stream.fileno(), self._on_readable, stream, buffer)
        self._streams.append(stream)

    def _on_readable(self, stream: IO[bytes], buffer: TailBuffer) -> None:
        try:
            data = os.read(stream.fileno(), 65536)
        except BlockingIOError:
            return
        except OSError:
            data = b""

        if data:
            buffer.write(data)
            self._merged.write(data)
        else:
            self._detach(stream)

    def _detach(self, stream: IO[bytes]) -> None:
        if stream in self._streams:
            self._streams.remove(stream)
            self._loop.remove_reader(stream.fileno())
            stream.close()

    async def _drain(self, timeout: float) -> None:
        deadline = self._loop.time() + timeout
        while self._streams and self._loop.time() < deadline:
            await asyncio.sleep(0.01)

    # -- lifecycle -------------------------------------------------------

    async def _watch(self) -> None:
        try:
            while (code := self._proc.poll()) is None:
                await asyncio.sleep(_FOREGROUND_POLL_SEC)

            await self._drain(0.05 if group_alive(self.pid) else 1.0)
            self._exit_code = code
            self._foreground_done.set()
            self._registry.changed()

            while group_alive(self.pid):
                await asyncio.sleep(_GROUP_POLL_SEC)
            await self._drain(0.5)
        finally:
            for stream in list(self._streams):
                self._detach(stream)
            if self._timer is not None:
                self._timer.cancel()
            if self._exit_code is None:
                self._exit_code = self._proc.poll() if self._proc.poll() is not None else -signal.SIGKILL
            self._foreground_done.set()
            self._group_done.set()
            self._registry.finished(self)

    def _result(self) -> BashResult:
        return BashResult(
            output=self._merged.text(),
            exit_code=self._exit_code if self._exit_code is not None else -1,
            stdout=self._stdout.text(),
            stderr=self._stderr.text(),
        )

    def _mark_read(self) -> None:
        if not self._foreground_done.is_set() or not in_live_cell() or self.read_in_cell:
            return
        self.read_in_cell = True
        self._registry.withdraw(self)

    # -- public API ------------------------------------------------------

    @property
    def running(self) -> bool:
        """True until the foreground command exits."""
        return not self._foreground_done.is_set()

    @property
    def finished(self) -> bool:
        """True once the whole process group (including background jobs) is gone."""
        return self._group_done.is_set()

    @property
    def exit_code(self) -> int | None:
        return self._exit_code if self._foreground_done.is_set() else None

    def poll(self) -> BashResult | None:
        if not self._foreground_done.is_set():
            return None
        self._mark_read()
        return self._result()

    def output(self) -> str:
        self._mark_read()
        return self._merged.text()

    def tail(self, n: int = 40) -> str:
        return "\n".join(self.output().splitlines()[-n:])

    async def wait(self) -> BashResult:
        await self._foreground_done.wait()
        self._mark_read()
        return self._result()

    def __await__(self) -> Any:
        return self.wait().__await__()

    def kill(self, sig: int = signal.SIGTERM) -> None:
        """Signal the whole process group; SIGKILL follows if it lingers."""
        self.killed = True
        signal_group(self.pid, sig)
        if sig != signal.SIGKILL:
            self._loop.call_later(_KILL_GRACE_SEC, self._force_kill)

    def reap(self) -> None:
        self._proc.poll()

    def _force_kill(self) -> None:
        if not self._group_done.is_set():
            signal_group(self.pid, signal.SIGKILL)

    def info(self) -> dict[str, Any]:
        return {"handle_id": self.handle_id, "pid": self.pid, "command": self.command, "running": self.running}

    def __repr__(self) -> str:
        state = "running" if self.running else f"exit={self._exit_code}"
        if not self.running and not self.finished:
            state += " (background jobs alive)"
        return f"<BashHandle {self.handle_id} pid={self.pid} {state} {self.command!r}>"
