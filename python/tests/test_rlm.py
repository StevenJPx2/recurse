from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))

from harness import FakeDaemon, HostError  # noqa: E402

PARENT = {"kind": "opencode-session", "id": "ses_abc"}


def child_info(name: str, status: str = "pending", session: dict | None = None) -> dict:
    return {
        "child_id": f"c_{name[:4]}0001",
        "name": name,
        "task": "Review things",
        "model": "anthropic/claude-sonnet-5",
        "status": status,
        "parent": PARENT,
        "session": session,
        "session_dir": "/abs/project",
        "depth": 1,
        "created_at": 1790000000000,
        "future_field": "ignored",
    }


class RlmTest(unittest.TestCase):
    def setUp(self) -> None:
        env = {"RECURSE_TARGET": json.dumps(PARENT), "RECURSE_DEPTH": "0", "RECURSE_MAX_DEPTH": "1"}
        self.kernel = FakeDaemon(env=env)
        self.kernel.handlers["rlm.spawn"] = lambda p: child_info(p["name"])
        self.kernel.handlers["rlm.list_subagents"] = lambda p: {
            "children": [child_info("api-reviewer", "running", {"kind": "opencode-session", "id": "ses_child"})]
        }
        self.kernel.handlers["rlm.delete_subagent"] = lambda p: {"child": child_info("api-reviewer", "deleted")}

    def tearDown(self) -> None:
        self.kernel.close()

    def test_env(self) -> None:
        self.assertEqual(self.kernel.ok("rlm.depth, rlm.max_depth, rlm.target"), repr((0, 1, PARENT)))

    def test_spawn(self) -> None:
        out = self.kernel.ok("h = await rlm.spawn('Review the API', name='api-reviewer')\nh.rlm_child_id, h.name, h.session_dir, h.model, h.status")
        self.assertEqual(out, repr(("c_api-0001", "api-reviewer", "/abs/project", "anthropic/claude-sonnet-5", "pending")))
        self.assertEqual(self.kernel.requests_for("rlm.spawn"), [{"task": "Review the API", "name": "api-reviewer"}])

        self.kernel.ok("await rlm.spawn('t', name='other', model='m/x')")
        self.assertEqual(self.kernel.requests_for("rlm.spawn")[1], {"task": "t", "name": "other", "model": "m/x"})

    def test_list_and_delete(self) -> None:
        out = self.kernel.ok("c = (await rlm.list_subagents())[0]\nc.session_name, c.status, c.active_session_id, c.rlm_child_id, c.depth")
        self.assertEqual(out, repr(("api-reviewer", "running", "ses_child", "c_api-0001", 1)))

        self.kernel.ok("await rlm.delete_subagent(c)")
        self.kernel.ok("await rlm.delete_subagent('api-reviewer')")
        self.kernel.ok("await rlm.delete_subagent('c_7f3a9b2c')")
        self.assertEqual(self.kernel.ok("(await rlm.delete_subagent(c)).status"), "'deleted'")
        self.assertEqual(
            self.kernel.requests_for("rlm.delete_subagent")[:3],
            [{"child_id": "c_api-0001"}, {"name": "api-reviewer"}, {"child_id": "c_7f3a9b2c"}],
        )

    def test_error_mapping(self) -> None:
        cases = {
            "unsupported_host": "UnsupportedHost",
            "depth_exceeded": "DepthExceeded",
            "limit_exceeded": "LimitExceeded",
            "not_found": "NotFound",
            "invalid_request": "InvalidRequest",
            "kernel_unavailable": "RecurseError",
        }
        for code, ename in cases.items():
            def fail(params: dict, code: str = code) -> None:
                raise HostError(code, f"nope {code}")

            self.kernel.handlers["rlm.spawn"] = fail
            error = self.kernel.execute("await rlm.spawn('t', name='x')")["error"]
            self.assertEqual((error["ename"], error["evalue"]), (ename, f"nope {code}"))

        out = self.kernel.ok(
            "try:\n    await rlm.spawn('t', name='x')\nexcept rlm.RecurseError as e:\n    r = (type(e).__name__, e.code)\nr"
        )
        self.assertEqual(out, repr(("RecurseError", "kernel_unavailable")))

        self.kernel.handlers["rlm.spawn"] = lambda p: (_ for _ in ()).throw(HostError("depth_exceeded", "deep"))
        out = self.kernel.ok(
            "from recurse_kernel import DepthExceeded\ntry:\n    await rlm.spawn('t', name='x')\nexcept DepthExceeded as e:\n    r = e.code\nr"
        )
        self.assertEqual(out, "'depth_exceeded'")

    def test_host_request(self) -> None:
        self.kernel.handlers["skills.list"] = lambda p: {"skills": [{"name": "core"}], "echo": p}
        self.assertEqual(self.kernel.ok("await rlm.host_request('skills.list')"), repr({"skills": [{"name": "core"}], "echo": {}}))

    def test_agent_message(self) -> None:
        self.assertEqual(self.kernel.ok("await agent_message.send('Found 2 issues.')"), "'agent.message:ses_parent:1'")
        self.kernel.ok("await agent_message.send('Check it', receiver_role='child', receiver_name='api-reviewer')")
        self.assertEqual(
            self.kernel.requests_for("agent_message.send"),
            [
                {"message": "Found 2 issues.", "receiver_role": "parent"},
                {"message": "Check it", "receiver_role": "child", "receiver_name": "api-reviewer"},
            ],
        )
        error = self.kernel.execute("await agent_message.send('x', receiver_role='child')")["error"]
        self.assertEqual(error["ename"], "InvalidRequest")


if __name__ == "__main__":
    unittest.main()
