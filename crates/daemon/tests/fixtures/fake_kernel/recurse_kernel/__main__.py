"""A fake recurse kernel for daemon tests. Speaks the §4 protocol with canned cells.

Cell code is a command:
  SLEEP n              sleep n seconds; honours interrupt
  HANG n               sleep n seconds; ignores interrupt
  EXIT                 exit the process with code 3
  PRINT text           write text to stdout
  BIG                  write 100 KiB to stdout
  ENV NAME             result = os.environ[NAME]
  CWD                  result = os.getcwd()
  SPAWN name task      host_request rlm.spawn
  SEND parent msg      host_request agent_message.send to the parent
  SEND child name msg  host_request agent_message.send to a child
  NOTICE handle_id     host_request notice.bash_finished
  WITHDRAW event_id    host_request notice.withdraw
  HOST method json     any host_request
  HANDLES              send a handles message
anything else          result = repr(code)
Host responses become the result (JSON) or an error whose ename is the error code.
"""

import json
import os
import sys
import threading
import time

protocol = os.fdopen(os.dup(1), "w", buffering=1)
write_lock = threading.Lock()
responses = {}
responses_ready = threading.Condition()
interrupted = threading.Event()
state = {"count": 0, "requests": 0}


def emit(message):
    with write_lock:
        protocol.write(json.dumps(message) + "\n")
        protocol.flush()


def host(method, params):
    with responses_ready:
        state["requests"] += 1
        request_id = f"r{state['requests']}"
    emit({"type": "host_request", "id": request_id, "method": method, "params": params})
    with responses_ready:
        while request_id not in responses:
            responses_ready.wait()
        return responses.pop(request_id)


def host_cell(method, params):
    response = host(method, params)
    if "error" in response and response["error"] is not None:
        error = response["error"]
        return "error", "", None, {"ename": error["code"], "evalue": error["message"], "traceback": ""}
    return "ok", "", json.dumps(response.get("result")), None


def sleep(seconds, honour_interrupt):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if honour_interrupt and interrupted.is_set():
            return "interrupted", "slept partially\n", None, {"ename": "KeyboardInterrupt", "evalue": "", "traceback": "KeyboardInterrupt"}
        time.sleep(0.02)
    return "ok", "slept\n", None, None


def send(rest):
    role, _, tail = rest.partition(" ")
    params = {"receiver_role": role}
    if role == "child":
        name, _, tail = tail.partition(" ")
        params["receiver_name"] = name
    params["message"] = tail
    return host_cell("agent_message.send", params)


def run(code):
    command, _, rest = code.partition(" ")
    if command == "SLEEP":
        return sleep(float(rest), True)
    if command == "HANG":
        return sleep(float(rest), False)
    if command == "EXIT":
        os._exit(3)
    if command == "PRINT":
        return "ok", rest + "\n", None, None
    if command == "BIG":
        return "ok", "x" * (100 * 1024), None, None
    if command == "ENV":
        return "ok", "", repr(os.environ.get(rest)), None
    if command == "CWD":
        return "ok", "", repr(os.getcwd()), None
    if command == "SPAWN":
        name, _, task = rest.partition(" ")
        return host_cell("rlm.spawn", {"name": name, "task": task})
    if command == "SEND":
        return send(rest)
    if command == "NOTICE":
        params = {"handle_id": rest, "pid": 777, "exit_code": 1, "command": "npm test"}
        return host_cell("notice.bash_finished", params)
    if command == "WITHDRAW":
        return host_cell("notice.withdraw", {"event_id": rest})
    if command == "HOST":
        method, _, params = rest.partition(" ")
        return host_cell(method, json.loads(params or "{}"))
    if command == "HANDLES":
        emit({"type": "handles", "handles": [{"handle_id": "h3", "pid": 777, "command": "npm test", "running": True}]})
        return "ok", "", None, None
    return "ok", "", repr(code), None


def execute(cell_id, code):
    state["count"] += 1
    started = time.monotonic()
    interrupted.clear()
    try:
        status, stdout, result, error = run(code)
    except Exception as exc:  # noqa: BLE001 - report every failure as a cell error
        status, stdout, result = "error", "", None
        error = {"ename": type(exc).__name__, "evalue": str(exc), "traceback": repr(exc)}
    emit({
        "type": "execute_result",
        "id": cell_id,
        "result": {
            "status": status,
            "stdout": stdout,
            "stderr": "",
            "result": result,
            "error": error,
            "execution_count": state["count"],
            "duration_ms": int((time.monotonic() - started) * 1000),
            "truncated": False,
        },
    })


def main():
    print("fake kernel starting", file=sys.stderr, flush=True)
    emit({"type": "ready", "pid": os.getpid(), "python": sys.version.split()[0], "protocol": 1})
    for line in sys.stdin:
        message = json.loads(line)
        kind = message["type"]
        if kind == "execute":
            threading.Thread(target=execute, args=(message["id"], message["code"]), daemon=True).start()
        elif kind == "interrupt":
            interrupted.set()
        elif kind == "host_response":
            with responses_ready:
                responses[message["id"]] = message
                responses_ready.notify_all()
        elif kind == "shutdown":
            os._exit(0)


main()
