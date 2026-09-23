# recurse Python API reference

Preloaded in every kernel namespace: `rlm`, `bash`, `agent_message`,
`skills`, `read_skill`, and one proxy per Python-backed skill. Types are also
importable: `from recurse_kernel import BashResult, ChildInfo, RecurseError, …`.

## bash

```python
bash(cmd: str, *, cwd: str | PathLike | None = None,
     env: Mapping[str, Any] | None = None, timeout: float | None = None) -> BashHandle
```

Runs `cmd` with `/bin/sh -c` in a new session (own process group). `cwd`
defaults to `os.getcwd()`; `env` is merged over `os.environ` (values are
`str()`-ed); stdin is `/dev/null`. `timeout` (seconds) sends SIGTERM to the
group, then SIGKILL 2 s later.

### BashHandle

| member | description |
|---|---|
| `handle_id: str` | `"h1"`, `"h2"`, … per kernel |
| `pid: int` | shell pid = process group id |
| `command: str` | |
| `running: bool` | foreground command still running |
| `finished: bool` | whole process group gone (background jobs included) |
| `exit_code: int \| None` | foreground exit code once `running` is False |
| `await h` / `await h.wait()` → `BashResult` | waits for the foreground command |
| `h.poll()` → `BashResult \| None` | `None` while running |
| `h.output()` → `str` | merged stdout+stderr so far (≤ 1 MiB tail) |
| `h.tail(n=40)` → `str` | last `n` lines of `output()` |
| `h.kill(sig=signal.SIGTERM)` | signal the group; SIGKILL follows after 2 s |

`exit_code` is negative when the shell was killed by a signal (`-15` = SIGTERM).
Output is decoded as UTF-8 with replacement.

Notices: when the group finishes and no cell has read the finished handle,
the session receives one `bash.finished` notice. Reads that count: `await h`,
`poll()` returning a result, `output()`/`tail()` after the foreground exit —
made from the currently executing cell (including tasks it awaits). Reads
from tasks left running from earlier cells do not count. `kill()` and
`await bash(...)` never notify.

### BashResult

```python
@dataclass(frozen=True)
class BashResult:
    output: str      # stdout and stderr interleaved in arrival order
    exit_code: int
    stdout: str
    stderr: str
```

## rlm

```python
rlm.depth: int                 # RECURSE_DEPTH (0 for a root session)
rlm.max_depth: int             # RECURSE_MAX_DEPTH (default 1)
rlm.target: dict | None        # {"kind": ..., "id": ...}

await rlm.spawn(task: str, *, name: str, model: str | None = None) -> ChildHandle
await rlm.list_subagents() -> list[ChildInfo]
await rlm.delete_subagent(child: ChildInfo | ChildHandle | str) -> ChildInfo
await rlm.host_request(method: str, /, **params) -> Any
```

`delete_subagent` accepts a handle, an info object, a child id (`"c_…"`, any
string containing `_`), or a name. `host_request` returns the daemon's raw
result (e.g. `await rlm.host_request("skills.list")`).

```python
@dataclass(frozen=True)
class ChildHandle:
    rlm_child_id: str
    name: str
    session_dir: str
    model: str | None
    status: str          # "pending" at admission

@dataclass(frozen=True)
class ChildInfo:
    child_id: str
    name: str
    task: str
    model: str | None
    status: str          # pending | running | failed | deleted
    parent: dict | None  # target
    session: dict | None # target, None while pending
    session_dir: str
    depth: int
    created_at: int      # unix ms
    # properties
    rlm_child_id: str        # = child_id
    session_name: str        # = name
    active_session_id: str | None  # = session["id"]
```

## agent_message

```python
await agent_message.send(message: str, receiver_role: str = "parent",
                         receiver_name: str | None = None) -> str   # event id
```

`receiver_role="parent"` works only in a child session. `"child"` requires
`receiver_name` naming one of your `running` children. Messages ≤ 32 KiB.

## Skills

```python
skills(include_hidden: bool = False) -> list[dict]   # {name, description, path, python, hidden}
read_skill(name: str, path: str = "SKILL.md") -> str  # file inside the skill directory
```

A skill with `python: module:function` in its frontmatter is bound as
`name.replace("-", "_")`. The first call imports the module (the skill
directory is on `sys.path`) and calls the function; an async function's
coroutine is returned, so `await` it.

## Errors

```python
class RecurseError(Exception):   # .code: str, .message: str
class UnsupportedHost(RecurseError)   # "unsupported_host"
class DepthExceeded(RecurseError)     # "depth_exceeded"
class LimitExceeded(RecurseError)     # "limit_exceeded"
class NotFound(RecurseError)          # "not_found"
class InvalidRequest(RecurseError)    # "invalid_request"
```

Also available as `rlm.RecurseError`, `rlm.UnsupportedHost`, …. Other daemon
codes raise `RecurseError` with that `.code`.

## Cell results

Each `ipython` call returns `status` (`ok`, `error`, `interrupted`,
`timeout`), `stdout`, `stderr` (≤ 64 KiB, tail kept, `[recurse: N earlier
bytes truncated]` marker), `result` (`repr` of the last expression, ≤ 16 KiB),
`error` (`ename`, `evalue`, `traceback`), `execution_count`, `duration_ms`,
and `truncated`. The last non-None value is also stored in `_`.
