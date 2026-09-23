from __future__ import annotations

import os
import signal
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))

from harness import FakeDaemon  # noqa: E402


class KernelTestCase(unittest.TestCase):
    env: dict[str, str] | None = None

    def setUp(self) -> None:
        self.kernel = FakeDaemon(env=self.env)

    def tearDown(self) -> None:
        self.kernel.close()
        self.assertEqual(self.kernel.bad_lines, [], "kernel wrote non-JSON to the protocol stream")


class ExecuteTest(KernelTestCase):
    def test_ready(self) -> None:
        ready = self.kernel.ready
        self.assertEqual(set(ready), {"type", "pid", "python", "protocol"})
        self.assertEqual(ready["protocol"], 1)
        self.assertEqual(ready["pid"], self.kernel.proc.pid)

    def test_print_and_last_expression(self) -> None:
        result = self.kernel.execute("print('hi')\n1 + 1")
        self.assertEqual(result["status"], "ok")
        self.assertEqual(result["stdout"], "hi\n")
        self.assertEqual(result["stderr"], "")
        self.assertEqual(result["result"], "2")
        self.assertIsNone(result["error"])
        self.assertFalse(result["truncated"])
        self.assertEqual(result["execution_count"], 1)
        self.assertIsInstance(result["duration_ms"], int)

    def test_none_result_is_null(self) -> None:
        self.assertIsNone(self.kernel.ok("None"))
        self.assertIsNone(self.kernel.ok("x = 1"))

    def test_persistent_state_and_count(self) -> None:
        self.kernel.ok("x = 41\ndef inc(v):\n    return v + 1")
        result = self.kernel.execute("inc(x)")
        self.assertEqual(result["result"], "42")
        self.assertEqual(result["execution_count"], 2)
        self.assertEqual(self.kernel.ok("__name__"), "'__main__'")

    def test_top_level_await(self) -> None:
        code = "import asyncio\nawait asyncio.sleep(0.01)\nasync def f():\n    return 'done'\nawait f()"
        self.assertEqual(self.kernel.ok(code), "'done'")

    def test_error_traceback_shape(self) -> None:
        self.kernel.ok("1")
        result = self.kernel.execute("1/0")
        self.assertEqual(result["status"], "error")
        self.assertIsNone(result["result"])
        error = result["error"]
        self.assertEqual(error["ename"], "ZeroDivisionError")
        self.assertEqual(error["evalue"], "division by zero")
        self.assertTrue(error["traceback"].startswith("Traceback (most recent call last):\n  File \"<cell 2>\", line 1, in <module>"))
        self.assertTrue(error["traceback"].endswith("ZeroDivisionError: division by zero"))
        self.assertNotIn("recurse_kernel", error["traceback"])

    def test_error_in_function_from_earlier_cell(self) -> None:
        self.kernel.ok("def boom():\n    raise ValueError('bad')")
        error = self.kernel.execute("boom()")["error"]
        self.assertIn('File "<cell 1>", line 2, in boom', error["traceback"])
        self.assertIn("raise ValueError('bad')", error["traceback"])

    def test_syntax_error_and_system_exit(self) -> None:
        error = self.kernel.execute("def (:")["error"]
        self.assertEqual(error["ename"], "SyntaxError")
        self.assertIn("<cell 1>", error["traceback"])

        result = self.kernel.execute("raise SystemExit(3)")
        self.assertEqual(result["error"]["ename"], "SystemExit")
        self.assertEqual(self.kernel.ok("'alive'"), "'alive'")

    def test_fd_level_output_is_captured(self) -> None:
        result = self.kernel.execute("import os, sys\nos.system('echo hi; echo err >&2')\nos.write(1, b'raw\\n')\nprint('p', file=sys.stderr)")
        self.assertEqual(result["status"], "ok")
        self.assertEqual(result["stdout"], "hi\nraw\n")
        self.assertEqual(result["stderr"], "err\np\n")
        self.assertEqual(self.kernel.ok("input.__name__"), "'input'")

    def test_stdin_is_not_the_protocol(self) -> None:
        result = self.kernel.execute("input()")
        self.assertEqual(result["error"]["ename"], "EOFError")

    def test_clipping(self) -> None:
        result = self.kernel.execute("print('a' * 100_000 + 'END')\n'x' * 50_000")
        self.assertTrue(result["truncated"])
        self.assertLessEqual(len(result["stdout"].encode()), 64 * 1024)
        self.assertTrue(result["stdout"].startswith("[recurse: "))
        self.assertTrue(result["stdout"].endswith("END\n"))
        self.assertLessEqual(len(result["result"].encode()), 16 * 1024)
        self.assertTrue(result["result"].startswith("'xxx"))

        big = self.kernel.execute("raise ValueError('v' * 40_000 + 'TAIL')")
        self.assertEqual(big["error"]["ename"], "ValueError")
        self.assertLessEqual(len(big["error"]["traceback"].encode()), 16 * 1024)
        self.assertLessEqual(len(big["error"]["evalue"].encode()), 16 * 1024)
        self.assertTrue(big["error"]["traceback"].endswith("TAIL"))
        self.assertTrue(big["truncated"])

        deep = self.kernel.execute("def r(n):\n    return r(n + 1)\nr(0)")
        self.assertEqual(deep["error"]["ename"], "RecursionError")
        self.assertLessEqual(len(deep["error"]["traceback"].encode()), 16 * 1024)

    def test_interrupt_busy_loop(self) -> None:
        self.kernel.send({"type": "execute", "id": "loop", "code": "print('start')\nwhile True:\n    pass"})
        time.sleep(0.3)
        self.kernel.send({"type": "interrupt"})
        message = self.kernel.expect("execute_result")
        self.assertEqual(message["id"], "loop")
        self.assertEqual(message["result"]["status"], "interrupted")
        self.assertEqual(message["result"]["stdout"], "start\n")
        self.assertEqual(message["result"]["error"]["ename"], "KeyboardInterrupt")
        self.assertEqual(self.kernel.ok("'alive'"), "'alive'")

    def test_interrupt_await_and_sleep(self) -> None:
        for code in ("import asyncio\nawait asyncio.sleep(60)", "import time\ntime.sleep(60)"):
            self.kernel.send({"type": "execute", "id": "wait", "code": code})
            time.sleep(0.3)
            self.kernel.send({"type": "interrupt"})
            result = self.kernel.expect("execute_result")["result"]
            self.assertEqual(result["status"], "interrupted", code)
        self.assertEqual(self.kernel.ok("1"), "1")

    def test_sigint_to_pid(self) -> None:
        self.kernel.send({"type": "execute", "id": "loop", "code": "while True:\n    pass"})
        time.sleep(0.3)
        os.kill(self.kernel.proc.pid, signal.SIGINT)
        self.assertEqual(self.kernel.expect("execute_result")["result"]["status"], "interrupted")
        self.kernel.send({"type": "interrupt"})
        self.assertEqual(self.kernel.ok("2"), "2")

    def test_background_task_keeps_running_between_cells(self) -> None:
        self.kernel.ok("import asyncio\nticks = []\nasync def tick():\n    while True:\n        ticks.append(1)\n        await asyncio.sleep(0.02)\nt = asyncio.create_task(tick())")
        time.sleep(0.3)
        self.assertGreater(int(self.kernel.ok("len(ticks)")), 5)

    def test_unknown_message_logs(self) -> None:
        self.kernel.send({"type": "bogus"})
        log = self.kernel.expect("log")
        self.assertEqual(set(log), {"type", "level", "message"})
        self.assertEqual(self.kernel.ok("3"), "3")

    def test_shutdown_exits_zero(self) -> None:
        self.kernel.send({"type": "shutdown"})
        self.assertEqual(self.kernel.proc.wait(5), 0)


if __name__ == "__main__":
    unittest.main()
