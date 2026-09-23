"""fd-level stdout/stderr capture, so user code can never touch the protocol."""

from __future__ import annotations

import os
import select
import sys
import threading
import time
from dataclasses import dataclass

from .bounds import STREAM_LIMIT, TailBuffer
from .protocol import block_sigint_in_thread

_SETTLE_SEC = 0.25


class CapturedStream:
    """Points `fd` at a pipe drained by a thread into a bounded buffer."""

    def __init__(self, fd: int) -> None:
        read_fd, write_fd = os.pipe()
        os.dup2(write_fd, fd)
        os.close(write_fd)
        os.set_blocking(read_fd, False)

        self._fd = read_fd
        self._lock = threading.Lock()
        self._buffer = TailBuffer(STREAM_LIMIT)
        threading.Thread(target=self._drain, name=f"recurse-capture-{fd}", daemon=True).start()

    def _drain(self) -> None:
        block_sigint_in_thread()
        while True:
            select.select([self._fd], [], [])
            with self._lock:
                if not self._read_once():
                    return

    def _read_once(self) -> bool:
        """Read one chunk. False on EOF; True otherwise (including 'empty')."""
        try:
            data = os.read(self._fd, 65536)
        except BlockingIOError:
            return True
        if not data:
            return False
        self._buffer.write(data)
        return True

    def take(self) -> tuple[str, bool]:
        """Drain whatever is already in the pipe, then swap the buffer out."""
        deadline = time.monotonic() + _SETTLE_SEC
        with self._lock:
            while time.monotonic() < deadline:
                try:
                    data = os.read(self._fd, 65536)
                except BlockingIOError:
                    break
                if not data:
                    break
                self._buffer.write(data)

            buffer, self._buffer = self._buffer, TailBuffer(STREAM_LIMIT)
        return buffer.clipped()


@dataclass
class ProtocolIO:
    in_fd: int
    out_fd: int
    stdout: CapturedStream
    stderr: CapturedStream

    def take(self) -> tuple[str, str, bool]:
        for stream in (sys.stdout, sys.stderr):
            try:
                stream.flush()
            except Exception:
                pass

        out, out_clipped = self.stdout.take()
        err, err_clipped = self.stderr.take()
        return out, err, out_clipped or err_clipped


def setup_io() -> ProtocolIO:
    """Keep the original fds 0/1 for protocol; redirect 0 to /dev/null, 1/2 to pipes."""
    in_fd = os.dup(0)
    out_fd = os.dup(1)

    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    os.close(devnull)

    return ProtocolIO(in_fd, out_fd, CapturedStream(1), CapturedStream(2))
