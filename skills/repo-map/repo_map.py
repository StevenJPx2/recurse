"""Compact tree + summary of a repository (stdlib only)."""

from __future__ import annotations

import os
import subprocess
from collections import Counter
from pathlib import Path

SKIP_DIRS = {".git", "node_modules", "target", "__pycache__", ".venv", "venv", "dist", "build", ".mypy_cache", ".pytest_cache"}


def list_files(root: Path) -> list[str]:
    """Repo-relative POSIX paths; honors .gitignore via `git ls-files` when possible."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            capture_output=True,
            check=True,
            timeout=30,
        )
        return sorted({p for p in proc.stdout.decode("utf-8", "replace").split("\0") if p and (root / p).exists()})
    except (OSError, subprocess.SubprocessError):
        pass

    files: list[str] = []
    for current, dirs, names in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        base = Path(current).relative_to(root)
        files += [(base / name).as_posix() for name in names]
    return sorted(files)


def _tree(files: list[str], max_depth: int, max_entries: int) -> list[str]:
    counts: Counter[str] = Counter()
    for path in files:
        parts = path.split("/")
        for depth in range(1, min(len(parts), max_depth + 1)):
            counts["/".join(parts[:depth]) + "/"] += 1
        if len(parts) <= max_depth:
            counts[path] = 0

    lines: list[str] = []
    for entry in sorted(counts):
        depth = entry.rstrip("/").count("/")
        label = entry.rstrip("/").rsplit("/", 1)[-1]
        if entry.endswith("/"):
            lines.append(f"{'  ' * depth}{label}/ ({counts[entry]} files)")
        else:
            lines.append(f"{'  ' * depth}{label}")
        if len(lines) >= max_entries:
            lines.append(f"… ({len(counts) - max_entries} more entries)")
            break
    return lines


def main(path: str = ".", *, max_depth: int = 2, max_entries: int = 200) -> str:
    """Return a header line (file count, top extensions) followed by an indented tree."""
    root = Path(path).expanduser().resolve()
    files = list_files(root)
    extensions = Counter(Path(f).suffix or Path(f).name for f in files)
    top = ", ".join(f"{ext} {n}" for ext, n in extensions.most_common(8))

    header = f"{root.name or root}: {len(files)} files" + (f" — {top}" if top else "")
    return "\n".join([header, *_tree(files, max_depth, max_entries)])
