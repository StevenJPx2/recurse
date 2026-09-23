"""Load the tests that ship next to built-in Python skills."""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))

from harness import REPO_DIR  # noqa: E402

from recurse_kernel.skills import discover  # noqa: E402

SKILLS_DIR = REPO_DIR / "skills"


def load_tests(loader: unittest.TestLoader, tests: unittest.TestSuite, pattern: str | None) -> unittest.TestSuite:
    suite = unittest.TestSuite()
    for skill_dir in sorted(p for p in SKILLS_DIR.iterdir() if p.is_dir()):
        if any(skill_dir.glob("test_*.py")):
            suite.addTests(loader.discover(str(skill_dir), pattern="test_*.py", top_level_dir=str(skill_dir)))
    suite.addTests(loader.loadTestsFromTestCase(BuiltinSkillsTest))
    return suite


class BuiltinSkillsTest(unittest.TestCase):
    def test_builtin_frontmatter(self) -> None:
        skills = discover(str(SKILLS_DIR), lambda level, message: self.fail(message))
        self.assertEqual(set(skills), {"recurse", "core", "repo-map"})
        self.assertTrue(skills["recurse"].hidden)
        self.assertFalse(skills["core"].hidden)
        self.assertIsNone(skills["core"].python)
        self.assertEqual(skills["repo-map"].python, "repo_map:main")
        self.assertTrue((SKILLS_DIR / "core" / "references" / "python-api.md").is_file())
