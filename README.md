# recurse

Recursive language model (RLM) runtime for existing coding-agent hosts. The
model works inside a persistent Python kernel through one tool, `ipython`, and
reaches files, shell commands, skills, and child agents as code:

```python
checks = bash("npm test")                       # live handle; end the turn instead of blocking
review = await rlm.spawn("Review the public API", name="api-reviewer")
await agent_message.send("Found 2 issues", receiver_role="parent")
```

recurse follows the running architecture of
[sourcefed](https://github.com/StevenJPx2/sourcefed) and
[chauffeur](https://github.com/StevenJPx2/chauffeur): an auto-spawned loopback
daemon (Rust) owns kernels, the child registry, message routing, and a
replay-until-ack event stream. Adapters for OpenCode, Pi, MCP, and a CLI
expose `ipython` and create child agents as **native host sessions**. The
daemon never calls a model.

- [`ARCHITECTURE.md`](ARCHITECTURE.md): responsibilities, lifecycle, Python
  API, host behavior, configuration, trust model.
- [`docs/protocol.md`](docs/protocol.md): RPC, events, kernel protocol, skills,
  bounds (protocol version 1).
- [`protocol/fixtures/`](protocol/fixtures): canonical JSON that the Rust,
  Python, and TypeScript implementations all test against.

## Status

Early (0.1.0), not yet published to npm, crates.io, or Homebrew. The full RLM
loop has been run end to end in real hosts with `gpt-6-luna`: the parent's
`ipython` cell lists files and calls `rlm.spawn`; the adapter creates a native
child session with its own kernel; the child reads a file and replies with
`agent_message.send`; the reply arrives as a parent turn and is acked.

| Host | Children | Verified |
|---|---|---|
| OpenCode 2.0.15 | native sessions via `ctx.session.create` | across three `opencode run --standalone` processes (durable queue + session inbox) |
| Pi 0.86 | in-process `createAgentSession` | one `pi --mode rpc` process |
| MCP (`recurse mcp`) | not supported (`UnsupportedHost`) | protocol tests |
| CLI (`recurse exec`) | not supported | integration tests |

## Build

Requirements: Rust 1.85+, Node 22+, Python 3.11+ (stdlib only).

```sh
cargo build --release          # target/release/recurse
npm install && npm run build   # packages/client, adapters/opencode, adapters/pi
```

## Use

The adapters spawn `recurse daemon` on demand. Point them at the binary with
`RECURSE_BIN` (or put `recurse` on `PATH`).

**OpenCode**: load the plugin from a project or global plugins directory:

```ts
// .opencode/plugins/recurse.ts
export { default } from "/path/to/recurse/adapters/opencode/dist/index.js"
```

**Pi**: `pi -e /path/to/recurse/adapters/pi/dist/index.js`, or install the
package (`"pi": {"extensions": ["./dist/index.js"]}`).

**MCP**: `recurse mcp` as a stdio server.

**CLI**:

```sh
recurse exec -c 'from pathlib import Path; sorted(p.name for p in Path(".").iterdir())'
recurse skills get core --full
```

Strict mode (default) leaves the model only `ipython`; set `RECURSE_STRICT=0`
to keep the host's tools, or `RECURSE_ALLOW_TOOLS=todo,webfetch` to keep some.
See [Configuration](ARCHITECTURE.md#configuration) for every variable.

### Children and one-shot runs

A child runs whenever its host is running. With a long-lived host (the
OpenCode background service, an interactive Pi session, `pi --mode rpc`) the
parent → child → parent loop completes on its own. A one-shot
`opencode run --standalone` exits when the parent's turn ends: the child's
first prompt stays in OpenCode's durable session inbox and recurse's queue
keeps the reply, so resuming the child and then the parent
(`opencode run --standalone -s <id> …`) finishes the loop. `pi -p` runs
children in-process and cannot outlive the parent's turn; use RPC or
interactive mode for recursive work.

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace                                   # 41 tests (fake + real kernel)
(cd python && python3 -m unittest discover -s tests)     # 51 tests
npm run check                                            # build, typecheck, 66 tests
```

## Trust model

The kernel runs model-generated Python and shell commands with your user's
permissions, outside the host's per-tool permission prompts. It is a durable
control environment, not a sandbox. Use an external sandbox for untrusted
repositories, instructions, or third-party Python skills.

Inspired by the RLM programming model in
[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent/blob/main/packages/coding-agent/docs/rlm.md).

## License

MIT
