# claude-code-adapter-probe: Claude Code behind the native adapter contract

Standalone development probe for Claude Code’s headless session, completion and containment behavior. See the [Native Agent contract](../../docs/layers/layer-5-agent.md#claude-code-conversations).

`probe.py` drives the user's own installed `claude` the way an adapter would: no model, permission mode or settings chosen by the probe, a new empty temporary folder per run. It prints event kinds, result subtypes, short ids, exit codes and process-tree facts, never transcript content except its own harmless prompts' replies.

## Run it

From a clean environment, outside any sandbox (Claude Code needs its network, its keychain sign-in and `~/.claude`):

```bash
env -i HOME="$HOME" USER="$USER" LOGNAME="$LOGNAME" PATH="$PATH" SHELL="$SHELL" TMPDIR="$TMPDIR" LANG=en_US.UTF-8 python3 -I tools/claude-code-adapter-probe/probe.py lifecycle
```

A shell inside a Claude Code session (desktop or CLI) passes that session's host variables to every child `claude`: bare mode (`CLAUDE_CODE_SIMPLE`), the host's `ANTHROPIC_BASE_URL`, host-delegated sign-in refresh (`CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH`) and its entrypoint. A child then authenticates as the host's, not as the user's own CLI. Pastey's process does not carry them, but an adapter must not forward them either if Pastey is ever started from such a shell.

Experiments: `detect`, `lifecycle`, `outcomes`, `held-resume`, `trust`, `proxy-cancel <interrupt|sigint|sigterm|stdin-eof>`, `proxy-orphan [bare|watchdog]`; and, with a signed-in CLI, `cancel <method>`, `orphan`, `exclusivity`, `permission <none-host|stdio-host>`. The proxy experiments keep a turn busy with a per-run `UserPromptSubmit` hook (`sleep 60`, passed with `--settings`) instead of a Bash tool call, so they run without a model. `cancel`, `orphan` and `exclusivity` grant exactly `Bash(sleep 60)` for that run only.

Each run leaves its transcript in `~/.claude/projects/-private-var-folders-…-pastey-cc-probe-*`. Remove them with:

```bash
find ~/.claude/projects -maxdepth 1 -name '*pastey-cc-probe-*' -type d -exec rm -r {} +
```
