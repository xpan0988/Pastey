# claude-code-adapter-probe: Claude Code behind the native adapter contract

Research only. Nothing here is part of the Pastey app. It answers what Claude Code's official headless interface (`claude -p`, stream-json in and out) guarantees about the things Pastey's Native Agent lifecycle relies on: session identity, completion evidence, cancellation, process containment and exclusivity. The conclusions are in [Claude Code adapter validation](../../docs/research/claude-code-adapter-validation.md).

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

## Evidence, 2026-11-04, MacBook arm64, macOS 26.5.2 (25F84), Claude Code 2.1.292

**The CLI was signed out** (`claude auth status`: `loggedIn: false`). Every model request ended immediately with `Failed to authenticate: OAuth session expired and could not be refreshed`. Everything below is real CLI behaviour, but no model turn ran. The hook stand-in replaces a running tool.

| Experiment | Observed |
|---|---|
| `detect` | `2.1.292 (Claude Code)`; `claude agents --json` readable. |
| New session, `--session-id U` | `system/init` and `result` both carry `U`. Transcript at `~/.claude/projects/<encoded cwd>/U.jsonl`. |
| Auth failure shape | `result` with `subtype: "success"`, **`is_error: true`**, `stop_reason: "stop_sequence"`, after a synthetic assistant message. The transcript marks it `isApiErrorMessage: true`, `error: "authentication_failed"`, model `<synthetic>`. Exit 1. |
| Same `--session-id U` again | Refused before anything: `Error: Session ID U is already in use.`, exit 1, no events. |
| `--resume U` | Continues under `U` (init and result), appends to the same transcript. |
| `--resume` of an unknown id | Only a `result` `error_during_execution`, `num_turns: 0`, no `init`; stderr `No conversation found with session ID`; exit 1. |
| Invalid option value | Exit 1, stderr names the option, no events. |
| `held-resume` | A live `-p` process holding `U` (idle, stdin open; registry `interactive`/`idle`). A second `-p --resume U` **was accepted and wrote `U`**; the holder's next prompt wrote too. Transcript: one parent entry with two children (a fork). |
| `initialize` control request (no prompt) | Answered at once, but `capabilities` lists only `ui_surface_v1`. `system/init` (after the first prompt) lists `interrupt_receipt_v1`, `interrupt_cancel_queued_v1`, `interrupt_send_now_v1`, `msg_lifecycle_v1`, … No `init` is sent before the first prompt. |
| `proxy-cancel interrupt` | `control_response` success in ~0.1 s; hook child killed; `result` `error_during_execution` (`is_error`, `stop_reason: null`); the process stays until stdin closes, then exit 1. Transcript: the prompt and an interruption entry. |
| `proxy-cancel sigint` | Same result event; hook child killed; exit 0. |
| `proxy-cancel sigterm` | Exit 143, no `result`, hook child killed, **nothing recorded** (no transcript). |
| `proxy-cancel stdin-eof` | Does **not** stop: the turn ran to the end (60.7 s), then exit 1. |
| `proxy-orphan` (bare) | Controlling process SIGKILLed mid-turn: `claude` stayed alive and `busy`, its child kept running; the turn finished ~75 s later (recorded), then `claude` exited. |
| `trust` (follow-up) | A never-trusted folder with two project hooks and one `.mcp.json` server, each touching a marker. `claude -p`: the **first event was `system/hook_started`, before `init`**; both hooks ran and the MCP server started, with no prompt. `claude --bg`: `Workspace not trusted…`, exit 1, no marker, nothing in the registry. |
| `proxy-orphan watchdog` | Same, with `claude` started under a lifeline watchdog (its own process group; SIGINT, then SIGTERM, then SIGKILL when the controller's pipe closes): turn interrupted and child killed within 1 s, `claude` gone within 5 s, no process left. |

Not run, because they need a model turn: a bounded reply and context continuity across resume; cancelling and orphaning a real Bash tool call (its process tree, background commands, subagents); exclusivity while the holder is mid-turn; permission requests (`permission_denied` without a host, `can_use_tool` with the `stdio` host); extra turns started by background work after a `result`. Windows was not run at all.
