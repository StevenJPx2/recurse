"""Skill discovery (RECURSE_SKILLS_PATH) and lazy Python-backed skill proxies."""

from __future__ import annotations

import importlib
import os
import re
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Callable

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
_PYTHON_RE = re.compile(r"^[A-Za-z_][\w.]*:[A-Za-z_]\w*$")

Logger = Callable[[str, str], None]


@dataclass(frozen=True)
class Skill:
    name: str
    description: str
    path: str
    python: str | None
    hidden: bool

    @property
    def directory(self) -> Path:
        return Path(self.path).parent


def parse_frontmatter(text: str) -> dict[str, str]:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}

    fields: dict[str, str] = {}
    for line in lines[1:]:
        if line.strip() == "---":
            return fields
        key, sep, value = line.partition(":")
        if sep and key.strip() and not key.startswith((" ", "\t", "#")):
            fields[key.strip()] = value.strip().strip("'\"")
    return {}


def load_skill(skill_md: Path, log: Logger) -> Skill | None:
    try:
        meta = parse_frontmatter(skill_md.read_text(encoding="utf-8"))
    except OSError as exc:
        log("warn", f"cannot read {skill_md}: {exc}")
        return None

    name = meta.get("name", "")
    python = meta.get("python") or None
    if not NAME_RE.match(name):
        log("warn", f"skipping skill at {skill_md}: invalid name {name!r}")
        return None
    if python is not None and not _PYTHON_RE.match(python):
        log("warn", f"skill {name}: invalid python entry {python!r} (want module:function)")
        python = None

    return Skill(
        name=name,
        description=meta.get("description", "")[:1024],
        path=str(skill_md.resolve()),
        python=python,
        hidden=meta.get("hidden", "false").lower() in ("true", "yes", "1"),
    )


def discover(search_path: str | None, log: Logger) -> dict[str, Skill]:
    """Entries are skill roots (containing skill dirs) or skill dirs; later entries shadow earlier."""
    found: dict[str, Skill] = {}
    for entry in (search_path or "").split(os.pathsep):
        if not entry:
            continue
        root = Path(entry).expanduser()
        candidates = [root / "SKILL.md"] if (root / "SKILL.md").is_file() else sorted(root.glob("*/SKILL.md"))
        for skill_md in candidates:
            skill = load_skill(skill_md, log)
            if skill is not None:
                found[skill.name] = skill
    return found


class SkillProxy:
    """Imports `module:function` on first call; returns awaitables unchanged."""

    def __init__(self, skill: Skill, log: Logger) -> None:
        self.skill = skill
        self._log = log
        self._function: Callable[..., Any] | None = None
        self.__doc__ = skill.description

    def _resolve(self) -> Callable[..., Any]:
        if self._function is None:
            module_name, _, attr = (self.skill.python or "").partition(":")
            try:
                self._function = getattr(importlib.import_module(module_name), attr)
            except Exception as exc:
                self._log("warn", f"skill {self.skill.name} failed to import: {exc}")
                raise
        return self._function

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        return self._resolve()(*args, **kwargs)

    def __repr__(self) -> str:
        return f"<skill {self.skill.name} ({self.skill.python}): {self.skill.description}>"


class SkillBook:
    def __init__(self, search_path: str | None, log: Logger) -> None:
        self._log = log
        self.skills = discover(search_path, log)

    def proxies(self, reserved: set[str]) -> dict[str, SkillProxy]:
        bound: dict[str, SkillProxy] = {}
        for skill in self.skills.values():
            if skill.python is None:
                continue
            ident = skill.name.replace("-", "_")
            if ident in reserved or not ident.isidentifier():
                self._log("warn", f"skill {skill.name}: name {ident!r} is reserved; not bound")
                continue
            directory = str(skill.directory)
            if directory not in sys.path:
                sys.path.insert(0, directory)
            bound[ident] = SkillProxy(skill, self._log)
        return bound

    def list(self, include_hidden: bool = False) -> list[dict[str, Any]]:
        """Skill metadata in the `skills.list` shape."""
        return [asdict(s) for s in self.skills.values() if include_hidden or not s.hidden]

    def read(self, name: str, path: str = "SKILL.md") -> str:
        """Return SKILL.md (or a file relative to the skill directory, e.g. references/…)."""
        skill = self.skills.get(name) or self.skills.get(name.replace("_", "-"))
        if skill is None:
            known = ", ".join(sorted(self.skills)) or "none"
            raise KeyError(f"unknown skill {name!r} (known: {known})")

        target = (skill.directory / path).resolve()
        if not target.is_relative_to(skill.directory.resolve()):
            raise ValueError(f"{path!r} is outside skill {name}")
        return target.read_text(encoding="utf-8")
