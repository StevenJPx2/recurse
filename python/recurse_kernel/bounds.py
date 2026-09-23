"""Size limits from docs/protocol.md (Bounds) and clipping helpers."""

from __future__ import annotations

STREAM_LIMIT = 64 * 1024
VALUE_LIMIT = 16 * 1024
BASH_LIMIT = 1024 * 1024

_MARKER_RESERVE = 80


def _skip_continuation(data: bytes) -> bytes:
    start = 0
    while start < min(len(data), 4) and (data[start] & 0xC0) == 0x80:
        start += 1
    return data[start:]


def clip_tail(data: bytes, limit: int, dropped: int = 0) -> tuple[str, bool]:
    """Keep the last `limit` bytes, prefixed by a marker line when clipped."""
    if dropped == 0 and len(data) <= limit:
        return data.decode("utf-8", "replace"), False

    kept = _skip_continuation(data[-(limit - _MARKER_RESERVE):])
    omitted = dropped + len(data) - len(kept)
    marker = f"[recurse: {omitted} earlier bytes truncated]\n"
    return marker + kept.decode("utf-8", "replace"), True


def clip_text_tail(text: str, limit: int) -> tuple[str, bool]:
    return clip_tail(text.encode("utf-8", "replace"), limit)


def clip_head(text: str, limit: int) -> tuple[str, bool]:
    """Keep the first `limit` bytes, followed by a marker when clipped."""
    raw = text.encode("utf-8", "replace")
    if len(raw) <= limit:
        return text, False

    kept = raw[: limit - _MARKER_RESERVE].decode("utf-8", "ignore")
    omitted = len(raw) - len(kept.encode("utf-8"))
    return f"{kept}\n[recurse: {omitted} more bytes truncated]", True


class TailBuffer:
    """Byte buffer that retains roughly the last `limit` bytes."""

    def __init__(self, limit: int) -> None:
        self.limit = limit
        self._data = bytearray()
        self._dropped = 0

    def write(self, data: bytes) -> None:
        self._data += data
        if len(self._data) > 2 * self.limit:
            cut = len(self._data) - self.limit
            del self._data[:cut]
            self._dropped += cut

    def clipped(self) -> tuple[str, bool]:
        return clip_tail(bytes(self._data), self.limit, self._dropped)

    def text(self) -> str:
        return self.clipped()[0]
