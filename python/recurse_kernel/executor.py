"""Compile and run cells in the persistent namespace; format errors."""

from __future__ import annotations

import ast
import inspect
import linecache
import os
import traceback
from types import CodeType, TracebackType
from typing import Any

_FLAGS = ast.PyCF_ALLOW_TOP_LEVEL_AWAIT
_KERNEL_DIR = os.path.dirname(os.path.abspath(__file__))


def cell_filename(count: int) -> str:
    return f"<cell {count}>"


class Executor:
    def __init__(self, namespace: dict[str, Any]) -> None:
        self.namespace = namespace
        self.count = 0

    async def run(self, code: str, count: int) -> Any:
        """Run `code`; return the value of a trailing expression (else None)."""
        filename = cell_filename(count)
        linecache.cache[filename] = (len(code), None, code.splitlines(True), filename)

        tree = compile(code, filename, "exec", flags=ast.PyCF_ONLY_AST | _FLAGS)
        last: ast.Expression | None = None
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            last = ast.Expression(tree.body.pop().value)

        await self._eval(compile(tree, filename, "exec", flags=_FLAGS))
        if last is None:
            return None

        value = await self._eval(compile(last, filename, "eval", flags=_FLAGS))
        if value is not None:
            self.namespace["_"] = value
        return value

    async def _eval(self, code: CodeType) -> Any:
        value = eval(code, self.namespace)
        if code.co_flags & inspect.CO_COROUTINE:
            value = await value
        return value


def _trim(tb: TracebackType | None) -> TracebackType | None:
    while tb is not None and tb.tb_frame.f_code.co_filename.startswith(_KERNEL_DIR):
        tb = tb.tb_next
    return tb


def _trim_chain(exc: BaseException) -> None:
    seen: set[int] = set()
    stack: list[BaseException | None] = [exc]
    while stack:
        current = stack.pop()
        if current is None or id(current) in seen:
            continue
        seen.add(id(current))
        current.__traceback__ = _trim(current.__traceback__)
        stack += [current.__cause__, current.__context__]


def describe_error(exc: BaseException) -> dict[str, str]:
    _trim_chain(exc)
    text = "".join(traceback.format_exception(exc)).rstrip("\n")
    return {"ename": type(exc).__name__, "evalue": str(exc), "traceback": text}
