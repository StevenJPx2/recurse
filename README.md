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
[chauffeur](https://github.com/StevenJPx2/chauffeur): an
auto-spawned loopback daemon (Rust) owns kernels, the child registry, message
routing, and a replay-until-ack event stream; adapters for OpenCode, Pi, MCP,
and a CLI expose `ipython` and create child agents as native host sessions.
The daemon never calls a model.

**Status:** under construction. The wire contract and architecture are
defined; implementations are landing next.

- [`ARCHITECTURE.md`](ARCHITECTURE.md): responsibilities, lifecycle, Python
  API, host behavior, configuration, trust model.
- [`docs/protocol.md`](docs/protocol.md): RPC, events, kernel protocol, skills,
  bounds (protocol version 1).
- [`protocol/fixtures/`](protocol/fixtures): canonical JSON that every
  implementation tests against.

Inspired by the RLM programming model in
[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent/blob/main/packages/coding-agent/docs/rlm.md).

## Trust model

The kernel runs model-generated Python and shell commands with your user's
permissions. It is a durable control environment, not a sandbox.

## License

MIT
