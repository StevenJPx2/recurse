from __future__ import annotations

import os
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))

from harness import REPO_DIR, FakeDaemon  # noqa: E402


def write_skill(root: Path, name: str, frontmatter: str, files: dict[str, str] | None = None) -> None:
    directory = root / name
    directory.mkdir(parents=True)
    (directory / "SKILL.md").write_text(f"---\n{frontmatter}\n---\n\n# {name}\n")
    for rel, text in (files or {}).items():
        (directory / rel).parent.mkdir(parents=True, exist_ok=True)
        (directory / rel).write_text(textwrap.dedent(text))


class SkillsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        user = Path(self.tmp.name, "user")
        project = Path(self.tmp.name, "project")

        write_skill(user, "release-audit", "name: release-audit\ndescription: old\npython: release_audit:main")
        write_skill(
            project,
            "release-audit",
            "name: release-audit\ndescription: Audit a release.\npython: release_audit:main\nhidden: false",
            {"release_audit.py": """
                import asyncio
                async def main(repository=".", target_version=None):
                    await asyncio.sleep(0)
                    return {"repository": repository, "version": target_version}
            """},
        )
        write_skill(
            project,
            "word-count",
            "name: word-count\ndescription: 'Count words.'\npython: counter:count",
            {"counter.py": "def count(text):\n    return len(text.split())\n"},
        )
        write_skill(project, "broken", "name: broken\ndescription: x\npython: missing_mod:run")
        write_skill(project, "notes", "name: notes\ndescription: Notes only.\nhidden: true", {"references/a.md": "ref A\n"})
        write_skill(project, "Bad_Name", "name: Bad_Name\ndescription: invalid")

        search = os.pathsep.join([str(REPO_DIR / "skills"), str(user), str(project)])
        self.kernel = FakeDaemon(env={"RECURSE_SKILLS_PATH": search})

    def tearDown(self) -> None:
        self.kernel.close()
        self.tmp.cleanup()

    def test_async_skill_proxy(self) -> None:
        out = self.kernel.ok("await release_audit(repository='.', target_version='0.4.0')")
        self.assertEqual(out, repr({"repository": ".", "version": "0.4.0"}))

    def test_sync_skill_proxy(self) -> None:
        self.assertEqual(self.kernel.ok("word_count('a b c')"), "3")

    def test_import_failure(self) -> None:
        result = self.kernel.execute("broken()")
        self.assertEqual(result["error"]["ename"], "ModuleNotFoundError")
        logs = [m["message"] for m in self.kernel.of_type("log")]
        self.assertTrue(any("skill broken failed to import" in m for m in logs), logs)

    def test_listing_and_reading(self) -> None:
        names = self.kernel.ok("sorted(s['name'] for s in skills())")
        self.assertEqual(names, repr(["broken", "core", "release-audit", "repo-map", "word-count"]))
        hidden = self.kernel.ok("sorted(s['name'] for s in skills(include_hidden=True) if s['hidden'])")
        self.assertEqual(hidden, repr(["notes", "recurse"]))
        shape = self.kernel.ok("sorted(skills()[0])")
        self.assertEqual(shape, repr(["description", "hidden", "name", "path", "python"]))
        self.assertEqual(self.kernel.ok("[s['description'] for s in skills() if s['name'] == 'release-audit']"), "['Audit a release.']")

        self.assertIn("# notes", self.kernel.ok("read_skill('notes')"))
        self.assertEqual(self.kernel.ok("read_skill('notes', 'references/a.md')"), "'ref A\\n'")
        self.assertEqual(self.kernel.execute("read_skill('nope')")["error"]["ename"], "KeyError")
        self.assertEqual(self.kernel.execute("read_skill('notes', '../../x')")["error"]["ename"], "ValueError")

    def test_builtin_repo_map_skill(self) -> None:
        out = self.kernel.execute(f"print(repo_map({str(REPO_DIR / 'skills')!r}))")
        self.assertEqual(out["status"], "ok", out)
        self.assertIn("core/", out["stdout"])


if __name__ == "__main__":
    unittest.main()
