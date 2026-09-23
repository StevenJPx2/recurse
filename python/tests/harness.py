"""A tiny fake daemon that drives the real kernel over the JSON-lines protocol."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Callable

PYTHON_DIR = Path(__file__).resolve().parent.parent
REPO_DIR = PYTHON_DIR.parent


class HostError(Exception):
    def __init__(self, code: str, message: str = "") -> None:
        super().__init__(message or code)
        self.code = code
        self.message = message or code


Handler = Callable[[dict[str, Any]], Any]


def _default_handlers() -> dict[str, Handler]:
    return {
        "notice.bash_finished": lambda p: {"event_id": f"bash.finished:k:{p['handle_id']}"},
        "notice.withdraw": lambda p: {"withdrawn": True},
        "agent_message.send": lambda p: {"event_id": "agent.message:ses_parent:1"},
        "skills.list": lambda p: {"skills": []},
    }


class FakeDaemon:
    def __init__(self, env: dict[str, str] | None = None, cwd: str | None = None) -> None:
        full_env = dict(os.environ)
        full_env["PYTHONPATH"] = str(PYTHON_DIR)
        full_env.setdefault("RECURSE_TARGET", json.dumps({"kind": "cli", "id": "test"}))
        full_env["RECURSE_KERNEL_PROTOCOL"] = "1"
        full_env.update(env or {})

        self.handlers = _default_handlers()
        self.requests: list[dict[str, Any]] = []
        self.messages: list[dict[str, Any]] = []
        self.bad_lines: list[bytes] = []
        self._inbox: queue.Queue[dict[str, Any]] = queue.Queue()
        self._cond = threading.Condition()
        self._write_lock = threading.Lock()

        self.proc = subprocess.Popen(
            [sys.executable, "-u", "-m", "recurse_kernel"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=full_env,
            cwd=cwd,
        )
        self._threads = [threading.Thread(target=f, daemon=True) for f in (self._read_stdout, self._read_stderr)]
        for thread in self._threads:
            thread.start()
        self.ready = self.expect("ready")

    # -- io --------------------------------------------------------------

    def _read_stdout(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            try:
                message = json.loads(raw)
            except ValueError:
                self.bad_lines.append(raw)
                continue
            with self._cond:
                self.messages.append(message)
                if message.get("type") == "host_request":
                    self.requests.append(message)
                self._cond.notify_all()
            if message.get("type") == "host_request":
                self._answer(message)
            else:
                self._inbox.put(message)

    def _read_stderr(self) -> None:
        assert self.proc.stderr is not None
        self.stderr = self.proc.stderr.read()

    def _answer(self, request: dict[str, Any]) -> None:
        handler = self.handlers.get(request["method"])
        reply: dict[str, Any] = {"type": "host_response", "id": request["id"]}
        try:
            if handler is None:
                raise HostError("invalid_request", f"unknown method {request['method']}")
            reply["result"] = handler(request["params"])
        except HostError as exc:
            reply["error"] = {"code": exc.code, "message": exc.message}
        self.send(reply)

    def send(self, message: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        with self._write_lock:
            self.proc.stdin.write((json.dumps(message) + "\n").encode())
            self.proc.stdin.flush()

    # -- waiting ---------------------------------------------------------

    def expect(self, kind: str, timeout: float = 10) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"no {kind!r} message within {timeout}s")
            try:
                message = self._inbox.get(timeout=remaining)
            except queue.Empty:
                continue
            if message.get("type") == kind:
                return message

    def execute(self, code: str, timeout: float = 10, cell_id: str | None = None) -> dict[str, Any]:
        cell_id = cell_id or f"x{time.monotonic_ns()}"
        self.send({"type": "execute", "id": cell_id, "code": code, "timeout_ms": int(timeout * 1000)})
        message = self.expect("execute_result", timeout)
        assert message["id"] == cell_id, message
        return message["result"]

    def ok(self, code: str, timeout: float = 10) -> str | None:
        result = self.execute(code, timeout)
        assert result["status"] == "ok", result
        return result["result"]

    def wait_for(self, predicate: Callable[[], bool], timeout: float = 10) -> bool:
        deadline = time.monotonic() + timeout
        with self._cond:
            while not predicate():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                self._cond.wait(remaining)
        return True

    def requests_for(self, method: str) -> list[dict[str, Any]]:
        with self._cond:
            return [r["params"] for r in self.requests if r["method"] == method]

    def wait_request(self, method: str, count: int = 1, timeout: float = 10) -> list[dict[str, Any]]:
        if not self.wait_for(lambda: len(self.requests_for(method)) >= count, timeout):
            raise TimeoutError(f"expected {count} {method} request(s), got {self.requests_for(method)}")
        return self.requests_for(method)

    def of_type(self, kind: str) -> list[dict[str, Any]]:
        with self._cond:
            return [m for m in self.messages if m.get("type") == kind]

    def close(self) -> None:
        if self.proc.poll() is None:
            try:
                self.send({"type": "shutdown"})
                self.proc.wait(5)
            except Exception:
                self.proc.kill()
                self.proc.wait()
        for thread in self._threads:
            thread.join(2)
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass
