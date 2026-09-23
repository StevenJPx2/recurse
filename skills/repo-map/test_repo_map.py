from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import repo_map  # noqa: E402


def make_tree(root: Path) -> None:
    for rel in ("README.md", "src/a.py", "src/b.py", "src/deep/x/y.py", "node_modules/pkg/index.js", "build.log"):
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("x\n")
    (root / ".gitignore").write_text("*.log\n")


class RepoMapTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name, "demo")
        self.root.mkdir()
        make_tree(self.root)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_walk_without_git(self) -> None:
        files = repo_map.list_files(self.root)
        self.assertIn("src/deep/x/y.py", files)
        self.assertNotIn("node_modules/pkg/index.js", files)

        out = repo_map.main(str(self.root))
        lines = out.splitlines()
        self.assertTrue(lines[0].startswith("demo: 6 files — .py 3"), lines[0])
        self.assertIn("src/ (3 files)", out)
        self.assertIn("  deep/ (1 files)", out)
        self.assertNotIn("y.py", out)

    @unittest.skipUnless(shutil.which("git"), "git not installed")
    def test_git_respects_gitignore(self) -> None:
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        (self.root / "node_modules").rename(self.root / "vendor")
        (self.root / ".gitignore").write_text("*.log\nvendor/\n")

        files = repo_map.list_files(self.root)
        self.assertNotIn("build.log", files)
        self.assertNotIn("vendor/pkg/index.js", files)
        self.assertIn(".gitignore", files)

    def test_max_entries(self) -> None:
        out = repo_map.main(str(self.root), max_depth=5, max_entries=2)
        self.assertIn("more entries", out.splitlines()[-1])


if __name__ == "__main__":
    unittest.main()
