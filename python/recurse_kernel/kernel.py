"""The kernel: dispatches daemon messages and runs cells on one event loop."""

from __future__ import annotations

import asyncio
import builtins
import contextvars
import os
import signal
import sys
import threading
import time
import traceback
import types
from typing import Any

from .bounds import VALUE_LIMIT, clip_head, clip_text_tail
from .capture import ProtocolIO
from .executor import Executor, describe_error
from .handles import HandleRegistry
from .protocol import Channel
from .rlm import AgentMessage, Rlm
from .skills import SkillBook
from .state import STATE, cell_var, in_live_cell


class Kernel:
    def __init__(self, loop: asyncio.AbstractEventLoop, io: ProtocolIO, channel: Channel) -> None:
        self.loop = loop
        self.io = io
        self.channel = channel
        self.registry = HandleRegistry(channel)
        self.rlm = Rlm(channel)
        self.agent_message = AgentMessage(channel)
        self.skills = SkillBook(os.environ.get("RECURSE_SKILLS_PATH"), channel.log)
        self.namespace = self._build_namespace()
        self.executor = Executor(self.namespace)

        self.cell_task: asyncio.Task[None] | None = None
        self._reported: set[str] = set()
        self._stopping = False
        self._main_ident = threading.main_thread().ident

    def _build_namespace(self) -> dict[str, Any]:
        module = types.ModuleType("__main__")
        namespace = module.__dict__
        namespace.update(
            __builtins__=builtins,
            rlm=self.rlm,
            bash=self.registry.bash,
            agent_message=self.agent_message,
            skills=self.skills.list,
            read_skill=self.skills.read,
        )
        namespace.update(self.skills.proxies(set(namespace) | set(dir(builtins))))
        sys.modules["__main__"] = module
        return namespace

    # -- dispatch --------------------------------------------------------

    def dispatch(self, message: dict[str, Any]) -> None:
        try:
            kind = message.get("type")
            if kind == "execute":
                self._start_cell(message)
            elif kind == "host_response":
                self.channel.resolve(message)
            elif kind == "shutdown":
                self.shutdown()
            elif kind != "interrupt":
                self.channel.log("warn", f"ignoring kernel message: {str(message)[:200]}")
        except Exception:
            self.channel.log("error", "kernel dispatch failed:\n" + traceback.format_exc())

    def request_interrupt(self) -> None:
        """Reader-thread side of `interrupt`: signal the main thread if a cell runs."""
        if STATE.running is not None and self._main_ident is not None:
            signal.pthread_kill(self._main_ident, signal.SIGINT)

    def on_sigint(self, signum: int, frame: Any) -> None:
        task = self.cell_task
        if task is None or task.done() or STATE.running is None:
            return
        if in_live_cell():
            raise KeyboardInterrupt
        self.loop.call_soon_threadsafe(task.cancel)

    # -- cells -----------------------------------------------------------

    def _start_cell(self, message: dict[str, Any]) -> None:
        cell_id = str(message.get("id", ""))
        if self.cell_task is not None and not self.cell_task.done():
            busy = {"ename": "KernelBusy", "evalue": "a cell is already running", "traceback": ""}
            self._send_result(cell_id, "error", None, busy, self.executor.count, time.monotonic(), False)
            return

        self.executor.count += 1
        count = self.executor.count
        context = contextvars.copy_context()
        context.run(cell_var.set, count)

        started = time.monotonic()
        task = self.loop.create_task(self._run_cell(cell_id, str(message.get("code", "")), count, started), context=context)
        task.add_done_callback(lambda done: self._after_cell(done, cell_id, count, started))
        self.cell_task = task

    async def _run_cell(self, cell_id: str, code: str, count: int, started: float) -> None:
        status, value, error = "ok", None, None
        STATE.running = count
        try:
            try:
                result = await self.executor.run(code, count)
                value = None if result is None else repr(result)
            finally:
                STATE.running = None
        except (KeyboardInterrupt, asyncio.CancelledError) as exc:
            status, error = "interrupted", describe_error(exc)
        except BaseException as exc:
            status, error = "error", describe_error(exc)

        self._send_result(cell_id, status, value, error, count, started, True)

    def _after_cell(self, task: asyncio.Task[None], cell_id: str, count: int, started: float) -> None:
        STATE.running = None
        if cell_id not in self._reported:
            error = {"ename": "KeyboardInterrupt", "evalue": "", "traceback": "KeyboardInterrupt"}
            self._send_result(cell_id, "interrupted", None, error, count, started, True)
        self._reported.discard(cell_id)

    def _send_result(
        self,
        cell_id: str,
        status: str,
        value: str | None,
        error: dict[str, str] | None,
        count: int,
        started: float,
        finish: bool,
    ) -> None:
        stdout, stderr, truncated = self.io.take() if finish else ("", "", False)
        if value is not None:
            value, clipped = clip_head(value, VALUE_LIMIT)
            truncated = truncated or clipped
        if error is not None:
            error["traceback"], clipped = clip_text_tail(error["traceback"], VALUE_LIMIT)
            error["evalue"] = clip_head(error["evalue"], VALUE_LIMIT)[0]
            truncated = truncated or clipped

        result = {
            "status": status,
            "stdout": stdout,
            "stderr": stderr,
            "result": value,
            "error": error,
            "execution_count": count,
            "duration_ms": int((time.monotonic() - started) * 1000),
            "truncated": truncated,
        }
        self.channel.send({"type": "execute_result", "id": cell_id, "result": result})

        if finish:
            self._reported.add(cell_id)
            self.registry.cell_finished()

    # -- lifecycle -------------------------------------------------------

    def run_forever(self) -> None:
        while not self._stopping:
            try:
                self.loop.run_forever()
            except KeyboardInterrupt:
                if self.cell_task is not None and not self.cell_task.done():
                    self.cell_task.cancel()
            except Exception:
                self.channel.log("error", "kernel loop error:\n" + traceback.format_exc())

    def shutdown(self) -> None:
        self._stopping = True
        self.registry.close()
        for task in asyncio.all_tasks(self.loop):
            task.cancel()
        self.loop.stop()
