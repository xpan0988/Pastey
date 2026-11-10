# codex-session-probe

Standalone development tools for distinguishing saved Codex history from a live turn. They use the app-server protocol without connecting to the desktop app’s private child. See [Layer 5](../../docs/layers/layer-5-agent.md#sessions-processes-and-tasks) for the product contract.

## Tools

- `probe.py` (read only): starts its own app-server and runs `thread/list` (state DB only), a metadata `thread/read` of the newest desktop thread, and `thread/loaded/list`. It prints counts and statuses only, never titles or content.
- `attach_poc.py`: starts its own app-server on a private Unix socket in a new temporary folder, connects two clients A and B, and shows what a second client of **the same server** can do with A's thread. `--turn` runs one small model turn that must ask for a command approval, and records who receives the request, who may answer it and what the other client sees. It never connects to the desktop's app-server or to the shared daemon.

```bash
python3 -I tools/codex-session-probe/probe.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex
python3 -I tools/codex-session-probe/attach_poc.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex --turn
```

With a Codex version that supports writer locks, these commands check cross-process exclusion and continuation in the same thread. Continuation uses model quota.

```bash
python3 -I tools/codex-session-probe/writer_lock_probe.py "$(command -v codex)" --cross-process
python3 -I tools/codex-session-probe/continuity_probe.py "$(command -v codex)"
```
