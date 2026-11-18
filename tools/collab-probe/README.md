# collab-probe

The M0 measurements for [Agent collaboration](../../docs/research/agent-collaboration-links.md). These are research tools and nothing here is part of the Pastey app.

The probe drives the user's own `codex` and `claude` the way Pastey's adapters do. Pastey's own choices stay out, apart from the per-run flags an experiment exists to measure. Every run uses a new temporary folder. The probe gives each child a clean environment (`HOME`, `USER`, `PATH`, `TMPDIR` and locale only).

Model turns use the user's quota. Reply text goes only to the output folder and is never printed.

## Experiments

```bash
python3 -I tools/collab-probe/probe.py codex-mcp
```

Codex `app-server` with a per-process MCP server passed by `-c mcp_servers.pastey.*`. It runs one turn for each `default_tools_approval_mode`: unset, `auto`, `prompt`, `writes` and `approve`. The probe never answers an MCP approval. It interrupts the turn instead, as Pastey would leave such a request to a person.

It records:
- what Codex asked the client;
- the tool call's status;
- what reached the server;
- whether `~/.codex/config.toml` changed.

```bash
python3 -I tools/collab-probe/probe.py claude-mcp
```

A new `claude -p` session, then two `--resume` runs with `--mcp-config`: one with `--allowedTools mcp__pastey__send` and one without. It records:
- whether the session id and transcript stay the same;
- permission denials;
- what reached the server;
- the names of the `CLAUDE*`/`CODEX*` variables the server got;
- whether the user's settings changed.

```bash
python3 -I tools/collab-probe/probe.py claude-inbound
python3 -I tools/collab-probe/probe.py claude-inbound refuse
```

A `claude -p` turn stays busy inside one foreground tool call, and a second `claude -p` session sends it one message with `SendMessage`.
- The receiver runs `python3 -c "import time; time.sleep(40)"` under `--allowedTools "Bash(python3 -c *)"`. Claude Code 2.1.318 blocks a standalone `sleep` and moves it to the background, which would end the turn early.
- With `refuse`, the receiver gets `--settings '{"crossSessionInbound":"refuse"}'`.

The probe records where the message token appears relative to the busy tool call's result and the turn's `result` events.

```bash
python3 -I tools/collab-probe/probe.py trailer codex warranted 20 --jobs 3
python3 -I tools/collab-probe/probe.py trailer claude unwarranted 20 --jobs 3
```

Independent turns over a small fixture, each prompt carrying the link note Pastey would add:
- In `warranted`, the task finds something the linked peer must change.
- In `unwarranted`, nothing concerns the peer.

The probe checks each final reply against the design's block rules:
- the block is at the end;
- `to` is a link alias;
- `kind` is `request` or `info`;
- a one-line subject;
- at most 8 KiB;
- the text renders faithfully;
- at most 3 blocks.

Claude runs pass `--no-session-persistence`, and Codex threads are `ephemeral`, so they don't add to the user's history.
