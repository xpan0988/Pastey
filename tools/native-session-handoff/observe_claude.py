"""Read-only observer of a Claude Code session as a second client (S3 PoC).

It watches one session through the two interfaces Claude Code offers to
other processes, and changes nothing:

- `claude agents --json`: whether a process holds the session, and its
  status (`busy`, `idle`, ...);
- the session's JSONL transcript, the file the official Agent SDK reads,
  which Claude Code appends to while a turn runs.

It prints, with timestamps, each registry status change and each new
transcript entry's kind (user prompt, assistant text, tool use, tool result,
end of turn), never the text unless --show-text is given. It sends nothing
to the session and cannot answer an approval.

    python3 -I observe_claude.py <workspace> <session-id> [--seconds N] [--show-text]
"""

import json
import os
import re
import subprocess
import sys
import time


def claude():
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = os.path.join(directory, "claude")
        if os.path.isfile(candidate):
            return candidate
    local = os.path.expanduser("~/.local/bin/claude")
    return local if os.path.isfile(local) else None


def registry_status(cli, session_id):
    try:
        output = subprocess.run(
            [cli, "agents", "--json"], capture_output=True, timeout=10, check=True
        ).stdout
        for entry in json.loads(output):
            if entry.get("sessionId") == session_id:
                return f"{entry.get('kind')}:{entry.get('status')}"
        return "not held"
    except Exception as error:  # noqa: BLE001 - report any failure as unknown
        return f"unknown ({type(error).__name__})"


def describe(entry, show_text):
    kind = entry.get("type")
    message = entry.get("message") or {}
    content = message.get("content")
    blocks = content if isinstance(content, list) else [{"type": "text", "text": content or ""}]
    parts = []
    for block in blocks:
        if not isinstance(block, dict):
            continue
        if block.get("type") == "text" and (block.get("text") or "").strip():
            parts.append("text" + (f"={block['text'][:80]!r}" if show_text else f"({len(block['text'])} chars)"))
        elif block.get("type") == "tool_use":
            parts.append(f"tool_use:{block.get('name')}")
        elif block.get("type") == "tool_result":
            parts.append("tool_result" + (":error" if block.get("is_error") else ""))
    stop = message.get("stop_reason")
    side = " sidechain" if entry.get("isSidechain") else ""
    return f"{kind}{side} {' '.join(parts) or '-'}" + (f" stop={stop}" if stop else "")


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    workspace = os.path.realpath(sys.argv[1])
    session_id = sys.argv[2]
    seconds = float(sys.argv[sys.argv.index("--seconds") + 1]) if "--seconds" in sys.argv else 60
    show_text = "--show-text" in sys.argv
    config = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    transcript = os.path.join(config, "projects", re.sub(r"[^A-Za-z0-9]", "-", workspace), f"{session_id}.jsonl")
    cli = claude()
    if not cli or not os.path.isfile(transcript):
        raise SystemExit("claude or the transcript was not found")
    started = time.time()
    offset = os.path.getsize(transcript)
    last_status = None
    print(f"observing from byte {offset}; transcript entries before now are skipped")
    while time.time() - started < seconds:
        status = registry_status(cli, session_id)
        if status != last_status:
            print(f"{time.time() - started:6.2f}s registry {status}")
            last_status = status
        size = os.path.getsize(transcript)
        if size > offset:
            with open(transcript, "rb") as file:
                file.seek(offset)
                chunk = file.read(size - offset)
            complete, _, _ = chunk.rpartition(b"\n")
            offset += len(complete) + (1 if complete else 0)
            for line in complete.splitlines():
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if entry.get("type") in ("user", "assistant"):
                    written = entry.get("timestamp", "")[11:23]
                    print(f"{time.time() - started:6.2f}s transcript (written {written}) {describe(entry, show_text)}")
        time.sleep(0.25)


if __name__ == "__main__":
    main()
