"""The kernel's real messages must match protocol/fixtures/kernel.json shapes."""

from __future__ import annotations

import json
import os
import sys
import unittest
from typing import Any

sys.path.insert(0, os.path.dirname(__file__))

from harness import REPO_DIR, FakeDaemon  # noqa: E402

FIXTURES = json.loads((REPO_DIR / "protocol" / "fixtures" / "kernel.json").read_text())


def shape(value: Any) -> Any:
    """Keys and JSON types, recursively; lists by their first element."""
    if isinstance(value, dict):
        return {key: shape(item) for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [shape(value[0])] if value else []
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, (int, float)):
        return "number"
    if value is None:
        return "null"
    return type(value).__name__


def fixture(kind: str, method: str | None = None) -> dict[str, Any]:
    for message in FIXTURES["from_kernel"]:
        if message["type"] == kind and (method is None or message.get("method") == method):
            return message
    raise KeyError((kind, method))


class FixtureShapeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.kernel = FakeDaemon()
        self.kernel.handlers["rlm.spawn"] = lambda p: FIXTURES["to_kernel"][2]["result"]

    def tearDown(self) -> None:
        self.kernel.close()

    def assertSameShape(self, actual: dict[str, Any], expected: dict[str, Any]) -> None:
        self.assertEqual(shape(actual), shape(expected))

    def test_ready(self) -> None:
        self.assertSameShape(self.kernel.ready, fixture("ready"))

    def test_execute_result(self) -> None:
        expected = FIXTURES["to_kernel"][0]
        self.kernel.send(expected)
        actual = self.kernel.expect("execute_result")
        self.assertSameShape(actual, fixture("execute_result"))
        self.assertEqual(actual["id"], "x1")
        self.assertEqual(
            {k: actual["result"][k] for k in ("status", "stdout", "stderr", "result", "error", "execution_count", "truncated")},
            {k: fixture("execute_result")["result"][k] for k in ("status", "stdout", "stderr", "result", "error", "execution_count", "truncated")},
        )

    def test_host_requests(self) -> None:
        self.kernel.ok("await rlm.spawn('Review the authentication flow for security issues', name='auth-reviewer')")
        self.kernel.ok("await agent_message.send('Found 2 issues.', receiver_role='parent')")
        self.kernel.ok("h = bash('exit 1')")
        self.kernel.wait_request("notice.bash_finished")
        self.kernel.ok("h.poll()")
        self.kernel.wait_request("notice.withdraw")

        by_method = {r["method"]: r for r in self.kernel.requests}
        for method in ("rlm.spawn", "agent_message.send", "notice.bash_finished", "notice.withdraw"):
            self.assertSameShape(by_method[method], fixture("host_request", method))
        self.assertEqual(by_method["rlm.spawn"]["params"], fixture("host_request", "rlm.spawn")["params"])

    def test_handles(self) -> None:
        self.kernel.ok("h = bash('sleep 0.2')")
        self.assertSameShape(self.kernel.of_type("handles")[0], fixture("handles"))

    def test_log(self) -> None:
        self.kernel.send({"type": "nonsense"})
        self.assertSameShape(self.kernel.expect("log"), fixture("log"))

    def test_error_fixture_from_rpc(self) -> None:
        rpc = json.loads((REPO_DIR / "protocol" / "fixtures" / "rpc.json").read_text())
        expected = rpc["kernel.execute.error"]["response"]["result"]
        self.kernel.ok("1")
        actual = self.kernel.execute("1/0")
        self.assertEqual(shape(actual), shape(expected))
        self.assertEqual(actual["error"]["ename"], expected["error"]["ename"])
        self.assertEqual(actual["error"]["evalue"], expected["error"]["evalue"])
        self.assertEqual(actual["execution_count"], expected["execution_count"])


if __name__ == "__main__":
    unittest.main()
