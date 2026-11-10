# native-session-handoff

Read-only development probe for Claude Code’s live registry and transcripts. See [Layer 5](../../docs/layers/layer-5-agent.md#sessions-processes-and-tasks) for the product boundary.

## Tool

`observe_claude.py <workspace> <session-id> [--seconds N] [--show-text]` is a read-only second client of one Claude Code session.
- It polls `claude agents --json` and tails the session transcript.
- It prints status changes and the kind of each new entry (prompt, text, tool use, tool result, end of turn), without content unless asked.
- It sends nothing and cannot answer an approval.

For Codex, `tools/codex-session-probe/attach_poc.py` is the two-client app-server PoC from D2.
