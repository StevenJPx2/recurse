"""Which cell is executing, and whether the current code runs inside it."""

from __future__ import annotations

from contextvars import ContextVar

cell_var: ContextVar[int | None] = ContextVar("recurse_cell", default=None)


class _State:
    running: int | None = None


STATE = _State()


def in_live_cell() -> bool:
    """True only inside the task tree of the cell that is executing right now."""
    running = STATE.running
    return running is not None and cell_var.get() == running
