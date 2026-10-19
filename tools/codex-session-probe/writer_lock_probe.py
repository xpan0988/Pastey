"""Does Codex refuse a second writer of one thread across processes?

Codex 0.163 keeps `~/.codex/thread-writer-locks/<thread-id>.lock` open for
each thread an app-server has loaded. This probe measures what that means,
through the official app-server protocol plus read-only lock tests:

1. Classify the lock the desktop holds on one of its threads, without taking
   an exclusive lock: a POSIX `F_GETLK` test (acquires nothing) and a
   non-blocking *shared* BSD `flock` attempt that is released at once.
2. Two separate app-servers A and B, started from the given Codex, both
   `thread/resume` one of Pastey's own earlier threads (originator
   "Pastey"). Never a desktop thread. No turn is started.
3. A releases the thread (`thread/unsubscribe`, then exits), and B tries
   again.

It prints ids shortened to 8 characters, results and error messages only:
no titles, previews, paths or content.

    python3 -I writer_lock_probe.py /opt/homebrew/bin/codex
"""

import errno
import json
import os
import struct
import subprocess
import sys
import time

try:
    import fcntl
except ImportError:  # Windows: the resume test below still runs.
    fcntl = None

LOCK_DIR = os.path.expanduser("~/.codex/thread-writer-locks")


class Server:
    def __init__(self, program, name):
        self.child = subprocess.Popen(
            [program, "app-server", "--listen", "stdio://"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        self.next_id = 0
        self.call("initialize", {"clientInfo": {"name": f"pastey-lock-probe-{name}", "version": "0"}, "capabilities": {}})
        self.notify("initialized", {})

    def notify(self, method, params):
        self.child.stdin.write(json.dumps({"method": method, "params": params}) + "\n")
        self.child.stdin.flush()

    def call(self, method, params, timeout=30):
        self.next_id += 1
        self.child.stdin.write(json.dumps({"id": self.next_id, "method": method, "params": params}) + "\n")
        self.child.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = self.child.stdout.readline()
            if not line:
                return {"error": f"app-server closed during {method}"}
            message = json.loads(line)
            if message.get("id") == self.next_id:
                if "error" in message:
                    return {"error": message["error"].get("message", "error")}
                return {"result": message.get("result", {})}
        return {"error": "timeout"}

    def close(self):
        try:
            self.child.stdin.close()
        except OSError:
            pass
        try:
            self.child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.child.terminate()
            self.child.wait(timeout=10)


def lock_state(path):
    """Classifies the lock on `path` without holding an exclusive one."""
    if not os.path.exists(path):
        return "no lock file"
    if fcntl is None:
        return "lock file present (not classified on this platform)"
    fd = os.open(path, os.O_RDONLY)
    try:
        # POSIX record lock test: F_GETLK reports a conflicting lock and
        # acquires nothing. struct flock on macOS: off_t start, off_t len,
        # pid_t pid, short type, short whence.
        request = struct.pack("qqihh", 0, 0, 0, fcntl.F_WRLCK, os.SEEK_SET)
        answer = fcntl.fcntl(fd, fcntl.F_GETLK, request)
        _, _, pid, kind, _ = struct.unpack("qqihh", answer)
        posix = f"posix lock held by pid {pid}" if kind != fcntl.F_UNLCK else "no posix lock"
        try:
            fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
            fcntl.flock(fd, fcntl.LOCK_UN)
            bsd = "no exclusive flock"
        except OSError as error:
            bsd = "exclusive flock held" if error.errno in (errno.EWOULDBLOCK, errno.EAGAIN) else f"flock error {error.errno}"
        return f"{posix}; {bsd}"
    finally:
        os.close(fd)


def short(thread_id):
    return thread_id[:8]


def main(program):
    report = {"codex": subprocess.run([program, "--version"], capture_output=True, text=True).stdout.strip()}

    # 1. The desktop's own locks, classified without interfering.
    held = sorted(name[:-5] for name in os.listdir(LOCK_DIR) if name.endswith(".lock") and not name.startswith("."))
    report["lock_files"] = len(held)
    report["existing_locks"] = {short(thread_id): lock_state(os.path.join(LOCK_DIR, thread_id + ".lock")) for thread_id in held}

    # 1b. Is a thread another process (the desktop) holds refused here?
    # A refused resume loads and writes nothing; on an unexpected success no
    # turn is started and the server exits at once.
    if held and "--cross-process" in sys.argv:
        probe = Server(program, "cross")
        foreign = probe.call("thread/resume", {"threadId": held[0]})
        report["resume_of_thread_held_elsewhere"] = foreign.get("error") or "ACCEPTED (unexpected)"
        probe.close()

    a = Server(program, "a")
    listed = a.call("thread/list", {"limit": 100, "useStateDbOnly": True})
    threads = (listed.get("result") or {}).get("data", [])
    own = [t for t in threads if t.get("originator") == "Pastey" and t["id"] not in held]
    if not own:
        report["error"] = "no idle Pastey-originated thread to test with"
        print(json.dumps(report, indent=1))
        a.close()
        return
    target = max(own, key=lambda t: t.get("updatedAt") or 0)["id"]
    lock_path = os.path.join(LOCK_DIR, target + ".lock")
    report["target"] = short(target)
    report["target_lock_before"] = lock_state(lock_path)

    # 2. A loads it; B tries the same thread while A holds it.
    resumed_a = a.call("thread/resume", {"threadId": target})
    report["a_resume"] = resumed_a.get("error") or ((resumed_a["result"].get("thread") or {}).get("status") or {}).get("type", "ok")
    report["target_lock_while_a_holds"] = lock_state(lock_path)
    b = Server(program, "b")
    resumed_b = b.call("thread/resume", {"threadId": target})
    report["b_resume_while_a_holds"] = resumed_b.get("error") or ((resumed_b["result"].get("thread") or {}).get("status") or {}).get("type", "ok")
    listed_b = b.call("thread/list", {"limit": 100, "useStateDbOnly": True})
    seen = [t for t in (listed_b.get("result") or {}).get("data", []) if t["id"] == target]
    report["b_sees_status"] = ((seen[0].get("status") or {}).get("type") if seen else "not listed")

    # 3. A releases; B tries again.
    report["a_unsubscribe"] = a.call("thread/unsubscribe", {"threadId": target}).get("error", "ok")
    time.sleep(1)
    report["target_lock_after_unsubscribe"] = lock_state(lock_path)
    a.close()
    time.sleep(1)
    report["target_lock_after_a_exit"] = lock_state(lock_path)
    retried = b.call("thread/resume", {"threadId": target})
    report["b_resume_after_a_released"] = retried.get("error") or ((retried["result"].get("thread") or {}).get("status") or {}).get("type", "ok")
    report["target_lock_while_b_holds"] = lock_state(lock_path)
    b.close()
    time.sleep(1)
    report["target_lock_after_b_exit"] = lock_state(lock_path)
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main(sys.argv[1])  # add --cross-process to also test a thread another process holds
