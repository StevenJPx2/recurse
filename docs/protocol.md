# recurse protocol (version 1)

This is the single source of truth for every wire contract in recurse. The Rust
daemon (`crates/protocol`), the Python kernel (`python/recurse_kernel`), and
the TypeScript client (`packages/client`) all implement it, and all three test
against the canonical examples in [`protocol/fixtures/`](../protocol/fixtures).
Change this file and the fixtures together.

Conventions:

- JSON field names are `snake_case` everywhere.
- Timestamps are integer Unix milliseconds (`*_ms` or `at`).
- Unknown fields are rejected by the daemon (serde `deny_unknown_fields`) on
  requests; clients ignore unknown fields on responses and events.
- Every string the daemon stores or relays is bounded (see [Bounds](#bounds)).

## 1. Transport

The daemon listens on loopback only, `127.0.0.1:18791` by default
(`RECURSE_DAEMON_PORT`). Clients reach it at `RECURSE_DAEMON_URL`
(default `http://127.0.0.1:18791`).

- `POST /rpc`: one request per HTTP request, JSON body ≤ 1 MiB.
- `GET /events?target=<base64url(JSON target)>`: Server-Sent Events.
- If `RECURSE_DAEMON_TOKEN` is set, both require
  `Authorization: Bearer <token>`; otherwise HTTP 401.

### 1.1 RPC envelope

Request:

```json
{"id": 7, "method": "kernel.execute", "params": {"target": {"kind": "opencode-session", "id": "ses_1"}, "code": "1 + 1"}}
```

Success (HTTP 200):

```json
{"id": 7, "result": {"status": "ok", "stdout": "", "stderr": "", "result": "2", "error": null, "execution_count": 1, "duration_ms": 3, "truncated": false}}
```

Failure (HTTP 400; 401 for auth; 500 only for daemon bugs):

```json
{"id": 7, "error": {"code": "busy", "message": "a cell is already running for opencode-session:ses_1"}}
```

`id` is optional and echoed. Error `code` is one of:

| code | meaning |
|---|---|
| `invalid_request` | malformed JSON, unknown method, unknown field, bad type |
| `unauthorized` | missing/incorrect bearer token |
| `not_found` | unknown target, child, or event |
| `busy` | a cell is already executing for this target |
| `kernel_unavailable` | Python kernel failed to start or died mid-request |
| `unsupported_host` | the target's host cannot perform the action (e.g. children on MCP/CLI) |
| `depth_exceeded` | `rlm.spawn` beyond `RECURSE_MAX_DEPTH` |
| `limit_exceeded` | a bound was exceeded (children, queue, payload size) |
| `internal` | daemon bug |

### 1.2 Target

```json
{"kind": "opencode-session", "id": "ses_abc"}
```

`kind` ∈ `opencode-session`, `pi-session`, `mcp`, `cli`. `id` is 1-256
characters. The target key is `"<kind>:<id>"`. A target owns exactly one
kernel, one event queue, and its children.

## 2. RPC methods

### `health`

Params: none (`{}` or omitted). Result:

```json
{"ok": true, "name": "recurse", "version": "0.1.0", "protocol": 1, "pid": 4242, "started_at": 1790000000000}
```

Clients must refuse to talk to a daemon whose `protocol` differs from theirs.

### `target.register`

Called by an adapter before a target's first kernel use (idempotent; later
calls update `cwd`, `host`, `model`).

```json
{
  "target": {"kind": "opencode-session", "id": "ses_abc"},
  "cwd": "/abs/project",
  "host": {"name": "opencode", "version": "2.0.14", "supports_children": true},
  "model": "anthropic/claude-sonnet-5"
}
```

`host.name` ∈ `opencode`, `pi`, `mcp`, `cli`. `model` is optional. Result:

```json
{
  "target": {"kind": "opencode-session", "id": "ses_abc"},
  "depth": 0,
  "parent": null,
  "child": null
}
```

For a session that was bound as a child (see `children.bind`), `depth` is the
parent's depth + 1, `parent` is the parent target, and `child` is its
`ChildInfo`.

### `kernel.execute`

```json
{"target": {"kind": "cli", "id": "laptop"}, "code": "print('hi')", "timeout_sec": 600}
```

- Starts the target's kernel if it is not running (the target must be
  registered; unregistered targets get `not_found`).
- One cell at a time per target: a concurrent call gets `busy`.
- `timeout_sec` default `RECURSE_CELL_TIMEOUT_SEC` (600), max 3600. On timeout
  the daemon interrupts the cell and returns `status: "timeout"` with the output
  so far. Kernel state survives.

Result (`CellResult`):

| field | type | notes |
|---|---|---|
| `status` | `"ok" \| "error" \| "timeout" \| "interrupted"` | |
| `stdout` | string | captured print/fd-1 output, ≤ 64 KiB |
| `stderr` | string | captured fd-2 output, ≤ 64 KiB |
| `result` | string or null | `repr()` of the last expression if not `None`, ≤ 16 KiB |
| `error` | `{ename, evalue, traceback}` or null | traceback ≤ 16 KiB |
| `execution_count` | integer | increments per executed cell |
| `duration_ms` | integer | |
| `truncated` | bool | any field was clipped |

### `kernel.interrupt`

`{"target": …}` → `{"interrupted": true|false}` (false when idle).

### `kernel.restart`

`{"target": …}` → `{"restarted": true}`. Python state is lost; the child
registry, event queue, and target registration are kept.

### `kernel.status`

`{"target": …}` → 

```json
{"state": "idle", "pid": 5151, "execution_count": 12, "started_at": 1790000000000, "cwd": "/abs/project", "handles": [{"handle_id": "h3", "pid": 777, "command": "npm test", "running": true}]}
```

`state` ∈ `absent`, `starting`, `idle`, `busy`, `dead`. `handles` lists live
`bash()` handles (reported by the kernel).

### `children.list`

`{"target": parent}` → `{"children": [ChildInfo, …]}`.

`ChildInfo`:

```json
{
  "child_id": "c_7f3a9b2c",
  "name": "auth-reviewer",
  "task": "Review the authentication flow for security issues",
  "model": "anthropic/claude-sonnet-5",
  "status": "running",
  "parent": {"kind": "opencode-session", "id": "ses_abc"},
  "session": {"kind": "opencode-session", "id": "ses_child"},
  "session_dir": "/abs/project",
  "depth": 1,
  "created_at": 1790000000000
}
```

`status` ∈ `pending` (admitted, no host session yet), `running` (bound),
`failed` (host could not create it; see `children.fail`), `deleted`. `session`
is null while `pending`. Names are unique per parent among non-deleted
children and match `^[a-z0-9][a-z0-9-]{0,62}$`.

### `children.bind`

Sent by the **parent's adapter** after it created the host session for a
`child.spawn` event.

```json
{"target": {"kind": "opencode-session", "id": "ses_abc"}, "child_id": "c_7f3a9b2c", "session": {"kind": "opencode-session", "id": "ses_child"}}
```

→ `{"child": ChildInfo}` (status `running`). From then on `target.register` for
`session` reports it as that child, and `agent_message` routing uses it.

### `children.fail`

`{"target": parent, "child_id": "…", "reason": "…"}` → `{"child": ChildInfo}`
(status `failed`). The daemon enqueues an `agent.message` event to the parent
from the child with `message` = `"[recurse] child failed to start: <reason>"`.

### `children.delete`

`{"target": parent, "child_id": "…"}` → `{"child": ChildInfo}` (status
`deleted`). Its session target is unbound; its own kernel is shut down.

### `events.ack`

`{"target": …, "event_ids": ["…"]}` → `{"acked": 2}`. Unknown ids are ignored.

### `events.list`

`{"target": …}` → `{"events": [Event, …]}` — everything queued and not yet
acked, oldest first. Used by MCP and CLI (no SSE).

### `skills.list`

`{"target": …}` (optional; its `cwd` adds project skills) →
`{"skills": [{"name", "description", "path", "python": "module:function" | null, "hidden": bool}]}`.

## 3. Events (`GET /events`)

Frames are `data: <json>\n\n`:

```json
{"type": "subscribed", "target": {"kind": "opencode-session", "id": "ses_abc"}}
{"type": "event", "target": {…}, "events": [Event, …]}
{"type": "heartbeat", "at": 1790000000000}
```

- On subscribe the daemon sends `subscribed`, then one `event` frame with
  everything queued and unacked (replay), then live events as they are queued.
- `heartbeat` every 15 s.
- Events stay queued until `events.ack`. Clients ack only after handling the
  event; on reconnect unacked events are replayed. Handlers must be idempotent
  on `event.id`.
- At most 256 queued events per target; beyond that the oldest non-actionable
  event is dropped, then the oldest overall.

`Event`:

```json
{
  "id": "agent.message:c_7f3a9b2c:3",
  "target": {"kind": "opencode-session", "id": "ses_abc"},
  "kind": "agent.message",
  "actionable": true,
  "at": 1790000000000,
  "text": "[recurse] message from child auth-reviewer (c_7f3a9b2c):\n\nFound 2 issues…",
  "payload": {"from": {"role": "child", "name": "auth-reviewer", "child_id": "c_7f3a9b2c"}, "message": "Found 2 issues…"}
}
```

- `actionable: true` → adapter starts/steers a turn with `text`
  (`session.prompt`, Pi `triggerTurn`).
- `actionable: false` → adapter adds `text` as context without a turn
  (`session.synthetic`, Pi `triggerTurn: false`), unless the kind is handled
  programmatically (`child.spawn`).

Kinds:

| kind | to | actionable | payload | id |
|---|---|---|---|---|
| `child.spawn` | parent | false (handled by adapter, never shown) | `{child: ChildInfo, prompt}` | `child.spawn:<child_id>` |
| `agent.message` | receiver | true | `{from: {role: "parent"\|"child", name, child_id}, message}` | `agent.message:<sender_key_or_child_id>:<seq>` |
| `bash.finished` | owner | true | `{handle_id, pid, exit_code, command}` | `bash.finished:<kernel_pid>:<handle_id>` |
| `kernel.exited` | owner | false | `{pid, exit_code, signal}` | `kernel.exited:<pid>` |

`child.spawn.payload.prompt` is the full first prompt the adapter sends to the
new session: the task plus a footer explaining that the child is a recurse
child named `<name>` and must reply with
`await agent_message.send(..., receiver_role="parent")`.

`bash.finished.text`:

```text
[recurse] Background command finished: pid 777, exit 1 (npm test). Inspect the saved handle with poll(), output(), or tail() and continue the task.
```

## 4. Kernel protocol (daemon ↔ Python)

The daemon starts one kernel per target:

```sh
$RECURSE_PYTHON -u -m recurse_kernel   # RECURSE_PYTHON default: python3 (≥ 3.11)
```

with `PYTHONPATH` prefixed by the kernel package directory, working directory
= the target's `cwd`, and environment:

- `RECURSE_TARGET` = JSON target
- `RECURSE_DEPTH`, `RECURSE_MAX_DEPTH`
- `RECURSE_SKILLS_PATH` = `:`-separated skill directories (see §5)
- `RECURSE_KERNEL_PROTOCOL=1`

Messages are single-line JSON on the kernel's **original** stdin/stdout. The
kernel must dup the original fd 1 for protocol use and point fds 1 and 2 at
capture pipes, so user code (`print`, `os.system`, C extensions) can never
corrupt the protocol stream. Kernel diagnostics go to the protocol as `log`.

Daemon → kernel:

```json
{"type": "execute", "id": "x1", "code": "…", "timeout_ms": 600000}
{"type": "interrupt"}
{"type": "host_response", "id": "r4", "result": {…}}
{"type": "host_response", "id": "r4", "error": {"code": "depth_exceeded", "message": "…"}}
{"type": "shutdown"}
```

Kernel → daemon:

```json
{"type": "ready", "pid": 5151, "python": "3.14.7", "protocol": 1}
{"type": "execute_result", "id": "x1", "result": CellResult}
{"type": "host_request", "id": "r4", "method": "rlm.spawn", "params": {…}}
{"type": "handles", "handles": [{"handle_id": "h3", "pid": 777, "command": "npm test", "running": true}]}
{"type": "log", "level": "warn", "message": "…"}
```

- `ready` must arrive within 10 s of spawn or the daemon kills the kernel
  (`kernel_unavailable`).
- The daemon forwards at most one `execute` at a time. `interrupt` (or SIGINT
  to the kernel pid) raises `KeyboardInterrupt` inside the running cell; the
  cell then reports `status: "interrupted"`. The daemon's timeout sends
  `interrupt`, waits 5 s, and if no `execute_result` arrives kills and marks the
  kernel `dead` (reporting `status: "timeout"`).
- The kernel may send `host_request` at any time, including while no cell is
  running (background tasks). It sends `handles` whenever the set of live
  handles changes.

### 4.1 Host requests (kernel → daemon)

| method | params | result |
|---|---|---|
| `rlm.spawn` | `{task, name, model?}` | `ChildInfo` (status `pending`) |
| `rlm.list_subagents` | `{}` | `{children: [ChildInfo]}` |
| `rlm.delete_subagent` | `{child_id}` or `{name}` | `{child: ChildInfo}` |
| `agent_message.send` | `{message, receiver_role: "parent"\|"child", receiver_name?}` | `{event_id}` |
| `notice.bash_finished` | `{handle_id, pid, exit_code, command}` | `{event_id}` |
| `notice.withdraw` | `{event_id}` | `{withdrawn: bool}` |
| `skills.list` | `{}` | `{skills: [...]}` |

Rules the daemon enforces:

- `rlm.spawn`: host must `supports_children` (else `unsupported_host`);
  `depth + 1 ≤ RECURSE_MAX_DEPTH` (default 1, i.e. roots may spawn, children
  may not); ≤ 16 non-deleted children per parent; unique valid `name`; `task`
  ≤ 32 KiB. Admission returns immediately with a `pending` ChildInfo and
  queues `child.spawn` to the parent target. `session_dir` = parent `cwd`.
  `model` defaults to the parent's registered `model`.
- `agent_message.send` with `receiver_role: "parent"` requires the sender to be
  a bound child; `"child"` requires `receiver_name` to name a non-deleted,
  `running` child of the sender. Message ≤ 32 KiB.
- `notice.withdraw` removes the event only if it is still queued and no SSE
  listener has been handed it yet; otherwise `withdrawn: false`.

## 5. Skills

A skill is a directory containing `SKILL.md` with YAML-ish frontmatter:

```markdown
---
name: release-audit
description: Audit a release candidate for missing changelog entries and version drift.
python: release_audit:main
hidden: false
---
```

- `name` matches `^[a-z0-9][a-z0-9-]{0,62}$`; `description` ≤ 1024 chars.
- `python` (optional) names `module:function`. The kernel prepends the skill
  directory to `sys.path` and binds `name` with `-` → `_` (e.g.
  `release_audit`) in the user namespace as a lazy proxy that imports on first
  call.
- Search path, later entries shadowing earlier ones by `name`: built-in
  (`skills/` shipped with recurse), `~/.config/recurse/skills`
  (`RECURSE_CONFIG_DIR`), then `<cwd>/.recurse/skills`.
- Only `name` + `description` of non-hidden skills are placed in the system
  prompt; the agent reads the full `SKILL.md` from Python when a task matches.

## Bounds

| item | limit |
|---|---|
| RPC body | 1 MiB |
| cell `code` | 256 KiB |
| stdout / stderr per cell | 64 KiB each (tail kept, head marker) |
| `result`, traceback | 16 KiB |
| event `text` | 64 KiB |
| queued events per target | 256 |
| children per parent | 16 |
| registered targets | 1024 |
| SSE frame | 1 MiB |
