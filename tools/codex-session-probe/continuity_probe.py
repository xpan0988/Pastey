"""Is a Codex thread continued from another process the same thread, with
its context, under the writer lock?

Two model turns, no tools, read-only sandbox, approvals disabled, in a new
empty temporary folder:

1. Process 1 starts a new thread and asks it to remember a random code word,
   then exits (which releases the thread's writer lock).
2. Process 2 resumes that thread id and asks for the code word, tagging the
   turn with a `clientUserMessageId`.
3. It records whether the thread id is unchanged, the number of turns before
   and after, whether the earlier turn is still in the history, whether the
   model's answer names the code word (context continuity), and whether the
   client message id is visible in the persisted history (for later
   reconciliation of an unknown outcome).

It prints ids shortened to 8 characters and booleans: no content except the
random code word it made up.

    python3 -I continuity_probe.py /opt/homebrew/bin/codex
"""

import json
import os
import secrets
import subprocess
import sys
import tempfile
import time

LOCK_DIR = os.path.expanduser("~/.codex/thread-writer-locks")


class Server:
    def __init__(self, program, cwd):
        self.child = subprocess.Popen(
            [program, "app-server", "--listen", "stdio://"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=cwd,
        )
        self.next_id = 0
        self.notifications = []
        self.call("initialize", {"clientInfo": {"name": "pastey-continuity-probe", "version": "0"}, "capabilities": {}})
        self.child.stdin.write(json.dumps({"method": "initialized", "params": {}}) + "\n")
        self.child.stdin.flush()

    def read(self):
        line = self.child.stdout.readline()
        return json.loads(line) if line else None

    def call(self, method, params, timeout=60):
        self.next_id += 1
        request_id = self.next_id
        self.child.stdin.write(json.dumps({"id": request_id, "method": method, "params": params}) + "\n")
        self.child.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            message = self.read()
            if message is None:
                return {"error": f"closed during {method}"}
            if message.get("id") == request_id and "method" not in message:
                return {"error": message["error"].get("message")} if "error" in message else {"result": message.get("result", {})}
            self.notifications.append(message)
        return {"error": "timeout"}

    def turn(self, thread_id, text, client_message_id, timeout=240):
        started = self.call("turn/start", {
            "threadId": thread_id,
            "input": [{"type": "text", "text": text}],
            "clientUserMessageId": client_message_id,
            "approvalPolicy": "never",
            "sandboxPolicy": {"type": "readOnly"},
        })
        if "error" in started:
            return {"error": started["error"]}
        turn_id = started["result"]["turn"]["id"]
        answer = ""
        deadline = time.time() + timeout
        backlog = list(self.notifications)
        self.notifications.clear()
        while time.time() < deadline:
            message = backlog.pop(0) if backlog else self.read()
            if message is None:
                return {"error": "closed during turn"}
            if message.get("method") == "item/completed":
                item = (message.get("params") or {}).get("item") or {}
                if item.get("type") == "agentMessage":
                    answer = item.get("text") or answer
            if message.get("method") == "turn/completed" and ((message.get("params") or {}).get("turn") or {}).get("id") == turn_id:
                return {"turn": turn_id[:8], "status": message["params"]["turn"].get("status"), "answer": answer}
        return {"error": "turn timeout"}

    def close(self):
        self.child.stdin.close()
        self.child.wait(timeout=15)


def history(server, thread_id):
    read = server.call("thread/read", {"threadId": thread_id, "includeTurns": True})
    thread = (read.get("result") or {}).get("thread") or {}
    return thread.get("turns") or []


def main(program):
    word = "pastey-" + secrets.token_hex(3)
    report = {"codex": subprocess.run([program, "--version"], capture_output=True, text=True).stdout.strip(), "code_word": word}
    folder = tempfile.mkdtemp(prefix="pastey-continuity-")

    first = Server(program, folder)
    started = first.call("thread/start", {"cwd": folder})
    if "error" in started:
        report["thread_start"] = started["error"]
        print(json.dumps(report, indent=1)); first.close(); return
    thread_id = started["result"]["thread"]["id"]
    report["thread"] = thread_id[:8]
    report["turn_1"] = first.turn(thread_id, f"Remember this code word for later: {word}. Reply with only: ok", "pastey-probe-1")
    report["lock_held_by_process_1"] = os.path.exists(os.path.join(LOCK_DIR, thread_id + ".lock"))
    first.close()
    time.sleep(1)
    report["lock_released_after_exit"] = not os.path.exists(os.path.join(LOCK_DIR, thread_id + ".lock"))

    second = Server(program, folder)
    resumed = second.call("thread/resume", {"threadId": thread_id, "cwd": folder})
    report["resume"] = resumed.get("error") or "ok"
    resumed_id = ((resumed.get("result") or {}).get("thread") or {}).get("id")
    report["same_thread_id"] = resumed_id == thread_id
    before = history(second, thread_id)
    report["turns_before"] = len(before)
    report["turn_2"] = second.turn(thread_id, "What was the code word I asked you to remember? Reply with only the word.", "pastey-probe-2")
    report["context_continuous"] = word in (report["turn_2"].get("answer") or "")
    after = history(second, thread_id)
    report["turns_after"] = len(after)
    report["earlier_turn_kept"] = bool(before) and bool(after) and after[0].get("id") == before[0].get("id")
    text = json.dumps(after)
    report["client_message_id_persisted"] = "pastey-probe-2" in text
    second.close()
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main(sys.argv[1])
