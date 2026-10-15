"""Read-only D2 probe: what a separate Codex app-server can see of existing
desktop sessions, through the official app-server protocol only.

It launches the given Codex (for example the one bundled with the desktop
app), initializes as a client, lists threads from the state database without
repair, reads the newest desktop thread's history, and lists the threads
loaded in its own process. It prints counts, sources, status types and turn
statuses only: no titles, previews, paths or content. It starts no turn and
writes nothing.

    python3 -I probe.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex
"""

import json
import subprocess
import sys
from collections import Counter


def main(program):
    child = subprocess.Popen(
        [program, "app-server", "--listen", "stdio://"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    next_id = 0

    def call(method, params):
        nonlocal next_id
        next_id += 1
        child.stdin.write(json.dumps({"id": next_id, "method": method, "params": params}) + "\n")
        child.stdin.flush()
        while True:
            line = child.stdout.readline()
            if not line:
                raise SystemExit(f"app-server closed during {method}")
            message = json.loads(line)
            if message.get("id") == next_id:
                if "error" in message:
                    return {"error": message["error"].get("message", "error")}
                return message.get("result", {})

    call("initialize", {"clientInfo": {"name": "pastey-d2-probe", "version": "0"}, "capabilities": {}})
    child.stdin.write(json.dumps({"method": "initialized", "params": {}}) + "\n")
    child.stdin.flush()

    listed = call("thread/list", {"limit": 50, "useStateDbOnly": True})
    threads = listed.get("data", []) if "error" not in listed else []
    desktop = [thread for thread in threads if thread.get("originator") == "Codex Desktop"]
    read = None
    if desktop:
        newest = max(desktop, key=lambda thread: thread.get("updatedAt") or 0)
        read = call("thread/read", {"threadId": newest["id"], "includeTurns": True})
    turns = (read or {}).get("thread", {}).get("turns") or []
    loaded = call("thread/loaded/list", {})
    report = {
        "thread_list_error": listed.get("error"),
        "threads_listed": len(threads),
        "sources": Counter(str(thread.get("source")) for thread in threads),
        "status_types": Counter((thread.get("status") or {}).get("type") for thread in threads),
        "originators": Counter(str(thread.get("originator")) for thread in threads),
        "newest_desktop_thread_read": None if read is None else (read.get("error") or "ok"),
        "its_turns": len(turns),
        "its_items": sum(len(turn.get("items") or []) for turn in turns),
        "its_turn_statuses": Counter(turn.get("status") for turn in turns),
        "its_status_seen_here": ((read or {}).get("thread", {}).get("status") or {}).get("type"),
        "loaded_in_this_process": len(loaded.get("data", [])) if "error" not in loaded else loaded,
    }
    print(json.dumps(report, indent=1, default=dict))
    child.stdin.close()
    child.terminate()
    child.wait(timeout=10)


if __name__ == "__main__":
    main(sys.argv[1])
