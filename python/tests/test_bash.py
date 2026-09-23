from __future__ import annotations

import os
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))

from harness import FakeDaemon  # noqa: E402


def group_alive(pgid: int) -> bool:
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:  # macOS: group of not-yet-reaped zombies
        return True
    return True


class BashTest(unittest.TestCase):
    def setUp(self) -> None:
        self.kernel = FakeDaemon()

    def tearDown(self) -> None:
        self.kernel.close()
        self.assertEqual(self.kernel.bad_lines, [])

    def settle(self, seconds: float = 0.4) -> None:
        time.sleep(seconds)

    def test_await_bash(self) -> None:
        result = self.kernel.ok("r = await bash('echo hi; echo oops >&2; exit 3')\nr.exit_code, r.stdout, r.stderr, r.output")
        self.assertEqual(result, "(3, 'hi\\n', 'oops\\n', 'hi\\noops\\n')")
        self.settle()
        self.assertEqual(self.kernel.requests_for("notice.bash_finished"), [])

    def test_cwd_env_and_chdir(self) -> None:
        self.kernel.ok("import os, tempfile\nd = os.path.realpath(tempfile.mkdtemp())\nos.chdir(d)\nos.environ['RK_A'] = 'a'")
        out = self.kernel.ok("(await bash('pwd; echo $RK_A$RK_B', env={'RK_B': 'b'})).output == d + '\\nab\\n'")
        self.assertEqual(out, "True")
        self.assertEqual(self.kernel.ok("(await bash('pwd', cwd='/')).output"), "'/\\n'")

    def test_unawaited_handle_notifies_once(self) -> None:
        self.kernel.ok("h = bash('sleep 0.2; exit 1')\nh.pid")
        params = self.kernel.wait_request("notice.bash_finished")
        self.settle()
        self.assertEqual(len(self.kernel.requests_for("notice.bash_finished")), 1)
        self.assertEqual(params[0]["exit_code"], 1)
        self.assertEqual(params[0]["handle_id"], "h1")
        self.assertEqual(params[0]["command"], "sleep 0.2; exit 1")
        self.assertEqual(str(params[0]["pid"]), self.kernel.ok("h.pid"))

    def test_read_in_cell_before_finish_suppresses_notice(self) -> None:
        self.kernel.ok("h = bash('sleep 0.3; echo done')")
        self.assertEqual(self.kernel.ok("(await h).output"), "'done\\n'")
        self.settle(0.6)
        self.assertEqual(self.kernel.requests_for("notice.bash_finished"), [])

    def test_finish_during_same_cell_then_read(self) -> None:
        self.kernel.ok("import asyncio\nh = bash('true')\nawait asyncio.sleep(0.3)\nh.poll().exit_code")
        self.settle()
        self.assertEqual(self.kernel.requests_for("notice.bash_finished"), [])

    def test_poll_while_running_does_not_count(self) -> None:
        self.assertEqual(self.kernel.ok("h = bash('sleep 0.3')\nh.poll()"), None)
        self.kernel.wait_request("notice.bash_finished")

    def test_read_after_notice_withdraws(self) -> None:
        self.kernel.ok("h = bash('echo x')")
        self.kernel.wait_request("notice.bash_finished")
        self.settle(0.1)
        self.assertEqual(self.kernel.ok("h.tail()"), "'x'")
        self.kernel.ok("h.poll(); h.output()")
        withdrawals = self.kernel.wait_request("notice.withdraw")
        self.settle()
        self.assertEqual(withdrawals, [{"event_id": "bash.finished:k:h1"}])

    def test_detached_watcher_read_does_not_count(self) -> None:
        self.kernel.ok(
            "import asyncio\nh = bash('sleep 0.2')\n"
            "async def watch():\n    await h\n    h.poll()\n"
            "w = asyncio.create_task(watch())"
        )
        self.kernel.wait_request("notice.bash_finished")
        self.settle()
        self.assertEqual(self.kernel.requests_for("notice.withdraw"), [])

    def test_background_job_keeps_handle_live(self) -> None:
        self.kernel.ok("h = bash('echo fg; sleep 1 &')")
        self.assertEqual(self.kernel.ok("import asyncio\nawait asyncio.sleep(0.3)\nh.running, h.finished"), "(False, False)")
        live = self.kernel.of_type("handles")[-1]["handles"]
        self.assertEqual(live, [{"handle_id": "h1", "pid": int(self.kernel.ok("h.pid")), "command": "echo fg; sleep 1 &", "running": False}])
        self.assertEqual(self.kernel.requests_for("notice.bash_finished"), [])

        self.kernel.wait_request("notice.bash_finished", timeout=5)
        self.assertEqual(self.kernel.of_type("handles")[-1]["handles"], [])

    def test_handles_messages(self) -> None:
        self.kernel.ok("h = bash('sleep 0.2')")
        first = self.kernel.of_type("handles")[0]["handles"]
        self.assertEqual(first[0]["running"], True)
        self.assertEqual(set(first[0]), {"handle_id", "pid", "command", "running"})
        self.assertTrue(self.kernel.wait_for(lambda: self.kernel.of_type("handles")[-1]["handles"] == []))

    def test_kill_and_timeout(self) -> None:
        self.assertEqual(self.kernel.ok("h = bash('sleep 30')\nh.kill()\n(await h).exit_code"), "-15")
        self.assertEqual(self.kernel.ok("(await bash('sleep 30', timeout=0.2)).exit_code"), "-15")
        self.settle()
        self.assertEqual(self.kernel.requests_for("notice.bash_finished"), [])

    def test_repr(self) -> None:
        self.assertIn("running 'sleep 5'", self.kernel.ok("h = bash('sleep 5')\nh"))
        self.assertEqual(self.kernel.ok("await bash('printf ok')"), "BashResult(exit_code=0, output='ok')")

    def test_shutdown_kills_process_groups(self) -> None:
        pid = int(self.kernel.ok("h = bash('sleep 30 & sleep 30')\nh.pid"))
        self.assertTrue(group_alive(pid))
        self.kernel.send({"type": "shutdown"})
        self.assertEqual(self.kernel.proc.wait(5), 0)
        deadline = time.monotonic() + 3
        while group_alive(pid) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(group_alive(pid))


if __name__ == "__main__":
    unittest.main()
