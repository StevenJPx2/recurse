"""JSON-lines protocol channel on the kernel's original stdin/stdout."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import threading
from typing import Any, Callable

from .bounds import VALUE_LIMIT, clip_head
from .errors import from_wire


def block_sigint_in_thread() -> None:
    """Keep process-directed SIGINT on the main thread."""
    signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})


class Channel:
    def __init__(self, out_fd: int, loop: asyncio.AbstractEventLoop) -> None:
        self._fd = out_fd
        self._loop = loop
        self._lock = threading.Lock()
        self._pending: dict[str, asyncio.Future[Any]] = {}
        self._seq = 0

    def send(self, message: dict[str, Any]) -> None:
        text = json.dumps(message, ensure_ascii=False, separators=(",", ":"), default=str)
        view = memoryview((text + "\n").encode("utf-8", "replace"))

        # A KeyboardInterrupt mid-write would corrupt the stream.
        masked = threading.current_thread() is threading.main_thread()
        if masked:
            previous = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})
        try:
            with self._lock:
                while view:
                    view = view[os.write(self._fd, view):]
        finally:
            if masked:
                signal.pthread_sigmask(signal.SIG_SETMASK, previous)

    def log(self, level: str, message: str) -> None:
        self.send({"type": "log", "level": level, "message": clip_head(message, VALUE_LIMIT)[0]})

    async def request(self, method: str, params: dict[str, Any] | None = None) -> Any:
        self._seq += 1
        request_id = f"r{self._seq}"
        future: asyncio.Future[Any] = self._loop.create_future()
        self._pending[request_id] = future

        try:
            self.send({"type": "host_request", "id": request_id, "method": method, "params": params or {}})
            return await future
        finally:
            self._pending.pop(request_id, None)

    def resolve(self, message: dict[str, Any]) -> None:
        future = self._pending.get(str(message.get("id")))
        if future is None or future.done():
            self.log("warn", f"host_response for unknown request {message.get('id')!r}")
            return

        if message.get("error") is not None:
            future.set_exception(from_wire(message["error"]))
        else:
            future.set_result(message.get("result"))


def start_reader(
    in_fd: int,
    loop: asyncio.AbstractEventLoop,
    on_message: Callable[[dict[str, Any]], None],
    on_interrupt: Callable[[], None],
) -> None:
    """Read daemon messages on a thread so a busy cell can still be interrupted."""

    def post(message: dict[str, Any]) -> None:
        try:
            loop.call_soon_threadsafe(on_message, message)
        except RuntimeError:
            pass

    def run() -> None:
        block_sigint_in_thread()
        with open(in_fd, "rb", closefd=False) as stream:
            for raw in stream:
                if not raw.strip():
                    continue
                try:
                    message = json.loads(raw)
                except ValueError:
                    message = {"type": "_invalid", "raw": raw[:200].decode("utf-8", "replace")}
                if not isinstance(message, dict):
                    message = {"type": "_invalid", "raw": repr(message)[:200]}
                if message.get("type") == "interrupt":
                    on_interrupt()
                post(message)
        post({"type": "shutdown"})

    threading.Thread(target=run, name="recurse-stdin", daemon=True).start()
