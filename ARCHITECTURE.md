# recurse architecture

recurse brings the recursive language model (RLM) programming model to
existing coding-agent hosts (OpenCode, Pi, any MCP client, and a CLI). The
model works inside a **persistent Python kernel** and composes capabilities as
code: files, shell commands, skills, and **child agents** are all reached from
one tool, `ipython`.

It follows the same running architecture as sourcefed and chauffeur: a
long-lived loopback daemon that adapters auto-spawn, a small JSON RPC surface
on `POST /rpc`, a replay-until-ack event stream on `GET /events`, and
`Target {kind, id}` ownership. Wire contracts are in
[`docs/protocol.md`](docs/protocol.md) with canonical fixtures in
[`protocol/fixtures/`](protocol/fixtures).

## Division of responsibility

| Layer | Owns | Never does |
|---|---|---|
| **Host** (OpenCode, Pi, MCP client) | models, credentials, provider calls, transcripts, compaction, UI, permissions of its own tools | run Python |
| **Adapter** (`adapters/*`, `crates/mcp`) | the `ipython` tool, strict-mode tool pruning, guidance + skill metadata in the system prompt, creating native child sessions for `child.spawn`, injecting events as turns or context | hold kernel state |
| **Daemon** (`crates/daemon`) | one Python kernel per target, the child registry (depth, names, status), `agent_message` routing, background-command notices, the event queue, skills discovery, persistence | call models |
| **Kernel** (`python/recurse_kernel`) | the user namespace, `rlm`, `bash`, `agent_message`, Python-backed skills, handle tracking | talk to anything except the daemon |

The daemon never calls a model. A child agent is a **native host session**
created by the parent's adapter, so it inherits the host's models, auth,
tools, and transcript store.

## Layout

```text
Cargo.toml                 Rust workspace
crates/protocol            serde types for docs/protocol.md + fixture tests
crates/daemon              axum server, kernel manager, registry, queue, persistence
crates/cli                 `recurse` binary (daemon, mcp, exec, kernel, children, events, skills, health)
crates/mcp                 stdio MCP server, a client of the daemon
python/recurse_kernel      the kernel (stdlib only, Python ≥ 3.11)
skills/                    built-in skills: `recurse` (hidden stub), `core` (+ references)
protocol/fixtures          canonical JSON shared by Rust, Python, TypeScript tests
packages/client            TypeScript: protocol types, RPC, SSE subscribe+ack, auto-spawn
adapters/opencode          OpenCode V2 server plugin
adapters/pi                Pi extension
GUIDANCE.md                text adapters inject into the system prompt
```

## Lifecycle

1. **Spawn.** On load an adapter calls `health`. If nothing answers and
   `RECURSE_DAEMON_URL` is unset, it spawns `recurse daemon` detached
   (`RECURSE_BIN`, else a bundled binary, else `recurse` on `PATH`) and polls
   `health` every 150 ms for up to 10 s. A protocol mismatch is a hard error.
2. **Single instance.** The daemon takes `state_dir/daemon.lock` (pid file,
   stale-pid takeover). A second daemon exits with a clear message.
3. **Register.** Before a target's first kernel use the adapter calls
   `target.register` with the session's `cwd`, host capabilities, and model.
4. **Subscribe.** The adapter subscribes each live session to `/events`
   (on session start/first prompt, not only on first tool call) and acks each
   event after handling it.
5. **Execute.** `ipython` → `kernel.execute`. The daemon starts the kernel on
   demand in the target's `cwd`; state persists across turns and host
   compaction.
6. **Spawn a child.** Python `await rlm.spawn(task, name=…)` →
   `host_request rlm.spawn` → daemon admits a `pending` child and queues
   `child.spawn` to the parent target → the parent's adapter creates a native
   session, calls `children.bind`, subscribes the child target, and sends
   `payload.prompt` as the child's first prompt. The Python call returned at
   admission with the handle; results arrive only as `agent.message` events or
   files.
7. **Messages.** `await agent_message.send(msg, receiver_role="parent")` in the
   child → `agent.message` event to the parent's target → adapter injects it as
   a turn. Parent → child is the same with `receiver_role="child",
   receiver_name=…`.
8. **Background commands.** `h = bash("npm test")` without `await` keeps a live
   handle. When its process group finishes and no live cell has read it, the
   kernel sends `notice.bash_finished`; the daemon queues an actionable
   `bash.finished` event, which wakes an idle session or steers a busy one at
   its next turn boundary. Reading the handle from a live cell before delivery
   sends `notice.withdraw`.
9. **Shutdown.** `SIGTERM`/`SIGINT` stop accepting RPC, send `shutdown` to every
   kernel, wait ≤ 5 s, kill stragglers, flush state, release the lock.
   `RECURSE_IDLE_EXIT_SEC` (default 0 = never) exits after no RPC and no live
   kernels for that long.

Python state lives only in kernel processes. It survives turns, compaction,
and adapter/host restarts (the daemon keeps running), but not a daemon
restart or `kernel.restart`. The child registry, target registrations, and
event queue are persisted and survive both.

## Python API (preloaded in every kernel)

```python
# files and data: plain Python
from pathlib import Path
big = [p for p in Path(".").rglob("*.toml") if p.stat().st_size > 10_000]

# shell: each bash() is its own process (own process group), sharing the
# kernel's cwd and os.environ
result = await bash("npm run check")      # BashResult(output, exit_code, stdout, stderr)
checks = bash("npm test")                 # live BashHandle; end the turn instead of blocking
checks.pid; checks.poll(); checks.output(); checks.tail(40); await checks

# children
h = await rlm.spawn("Review the public API", name="api-reviewer", model=None)
h.rlm_child_id, h.name, h.session_dir, h.model
for c in await rlm.list_subagents():
    c.session_name, c.status, c.active_session_id
await rlm.delete_subagent(h)

# messages
await agent_message.send("Found 2 issues", receiver_role="parent")
await agent_message.send("Check the new test", receiver_role="child", receiver_name="api-reviewer")

# skills: Python-backed skills are bound by name
report = await release_audit(repository=".", target_version="0.4.0")

# typed host bridge
await rlm.host_request("skills.list")
```

Cells may use top-level `await`. The last expression's `repr` is returned.
The kernel's event loop keeps running between cells, so background tasks and
handles make progress while the agent is idle.

### Notice semantics

- A read (`await h`, `poll()`, `output()`, `tail()`) counts only while a cell
  is executing. Reads from detached tasks between turns do not count.
- Handle finishes, unread → kernel sends `notice.bash_finished` once.
- Read after the notice was queued but before an SSE listener took it →
  `notice.withdraw` drops it. A notice already delivered is never retracted.
- `await bash(...)` never produces a notice.

## Hosts

### OpenCode (`adapters/opencode`)

- `Plugin.define({ id: "recurse" })`; registers the `ipython` tool with
  `ctx.tool.transform`.
- `ctx.session.hook("context")` injects `GUIDANCE.md` plus the non-hidden skill
  list, and in **strict mode** (default; `RECURSE_STRICT=0` to disable) removes
  every tool except `ipython` and names in `RECURSE_ALLOW_TOOLS`.
- `child.spawn` → `ctx.session.create({ title, agent, model, location: {directory: session_dir}, metadata: {"recurse.parent", "recurse.child_id", "recurse.name"} })`
  → `children.bind` → subscribe child → `ctx.session.prompt`. Failure →
  `children.fail`.
- Actionable events → `ctx.session.prompt`; others → `ctx.session.synthetic`.

### Pi (`adapters/pi`)

- `pi.registerTool` for `ipython`; strict mode via `pi.setActiveTools(["ipython", …])`
  on `session_start`.
- Children run **in-process** through the Pi SDK `createAgentSession({ cwd,
  sessionManager })`; the child session loads installed extensions, including
  recurse, so the child gets its own kernel. The parent adapter binds it and
  calls `session.prompt(payload.prompt)`.
- Injection: `pi.sendMessage({customType: "recurse", content: text, display: true}, {triggerTurn: actionable, deliverAs: "steer"})`.

### MCP (`crates/mcp`, `recurse mcp`)

- Stdio MCP server that is a client of the daemon (so it never contends for
  the state lock). Target `{kind: "mcp", id: RECURSE_TARGET_ID or a per-process id}`.
- Tools: `ipython`, `kernel_status`, `kernel_restart`, `kernel_interrupt`,
  `events_read`, `events_ack`. Hosts are registered with
  `supports_children: false`, so `rlm.spawn` raises `UnsupportedHost`.
  MCP cannot hide the host's own tools; strict mode does not apply.

### CLI

`recurse exec` runs cells against a `cli` target (`--target-id`, default
hostname); `recurse events follow` prints its events. No children.

## Configuration

| env | default | |
|---|---|---|
| `RECURSE_DAEMON_URL` | `http://127.0.0.1:18791` | set → adapters never spawn |
| `RECURSE_DAEMON_PORT` | `18791` | |
| `RECURSE_DAEMON_TOKEN` | unset | bearer token for `/rpc` and `/events` |
| `RECURSE_BIN` | bundled or `recurse` on PATH | daemon binary adapters spawn |
| `RECURSE_STATE_DIR` | `$XDG_STATE_HOME/recurse` or `~/.local/state/recurse` | lock, `registry.json`, `events.json`, `daemon.log` |
| `RECURSE_CONFIG_DIR` | `~/.config/recurse` | user skills in `skills/` |
| `RECURSE_PYTHON` | `python3` | ≥ 3.11 |
| `RECURSE_KERNEL_PATH` | bundled `python/` | directory containing `recurse_kernel` |
| `RECURSE_SKILLS_DIR` | bundled `skills/` | built-in skills |
| `RECURSE_MAX_DEPTH` | `1` | roots may spawn; raise for deeper recursion |
| `RECURSE_CELL_TIMEOUT_SEC` | `600` | per-cell default |
| `RECURSE_IDLE_EXIT_SEC` | `0` | daemon idle exit |
| `RECURSE_STRICT` | `1` | adapters expose only `ipython` |
| `RECURSE_ALLOW_TOOLS` | empty | comma-separated host tools kept in strict mode |

Daemon logs go to `state_dir/daemon.log` (and stderr when run in the
foreground). State files are written atomically (tmp + rename) with modes
`0700`/`0600`.

## Trust model

The kernel runs model-generated Python and shell commands with the daemon's
operating-system permissions, outside the host's per-tool permission prompts
(the host can still gate the `ipython` tool as a whole). It is a durable
control environment, not a sandbox. Use an external sandbox for untrusted
repositories, instructions, or third-party Python skills.
