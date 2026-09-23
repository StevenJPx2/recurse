"""Kernel process entry point."""

from __future__ import annotations

import asyncio
import os
import platform
import signal
import sys

from .capture import setup_io
from .kernel import Kernel
from .protocol import Channel, start_reader

PROTOCOL_VERSION = 1


def _install_signal_wakeup(loop: asyncio.AbstractEventLoop, kernel: Kernel) -> None:
    """SIGINT handler plus a wakeup fd so a signal always wakes an idle loop."""
    signal.signal(signal.SIGINT, kernel.on_sigint)

    read_fd, write_fd = os.pipe()
    os.set_blocking(read_fd, False)
    os.set_blocking(write_fd, False)
    signal.set_wakeup_fd(write_fd, warn_on_full_buffer=False)

    def drain() -> None:
        try:
            os.read(read_fd, 4096)
        except BlockingIOError:
            pass

    loop.add_reader(read_fd, drain)


def main() -> None:
    io = setup_io()
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    channel = Channel(io.out_fd, loop)
    kernel = Kernel(loop, io, channel)
    _install_signal_wakeup(loop, kernel)
    start_reader(io.in_fd, loop, kernel.dispatch, kernel.request_interrupt)

    channel.send({"type": "ready", "pid": os.getpid(), "python": platform.python_version(), "protocol": PROTOCOL_VERSION})
    kernel.run_forever()

    try:
        sys.stdout.flush()
        sys.stderr.flush()
    finally:
        os._exit(0)
