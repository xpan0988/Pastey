"""Can Claude Code be a second native capability behind Pastey's adapter
contract? A disposable-workspace probe of the official headless interface.

Research only; nothing here is part of the Pastey app. It drives the user's
own installed `claude` exactly as an adapter would: `claude -p` with
stream-json input and output, the user's own configuration and sign-in,
no model, permission mode or settings chosen by Pastey. Every run happens in
a new empty temporary folder.

Each experiment prints facts only: event kinds, result subtypes, short ids,
exit codes, process-tree observations and timings. Reply text is printed
only for the probe's own harmless prompts, cut to 80 characters.

    python3 -I probe.py detect
    python3 -I probe.py lifecycle        # new session, bounded reply, exact resume
    python3 -I probe.py outcomes         # failed and definite non-start shapes
    python3 -I probe.py cancel <interrupt|sigint|sigterm|stdin-eof>
    python3 -I probe.py orphan [bare|watchdog]  # the controlling process disappears
    python3 -I probe.py exclusivity      # two clients on one session id
    python3 -I probe.py permission <none-host|stdio-host>

Without a model turn (a signed-out CLI still runs these):

    python3 -I probe.py held-resume      # a live -p process holds the session
    python3 -I probe.py trust            # untrusted project hooks and .mcp.json
    python3 -I probe.py proxy-cancel <interrupt|sigint|sigterm|stdin-eof>
    python3 -I probe.py proxy-orphan [bare|watchdog]

`watchdog` runs claude under a lifeline watchdog that interrupts it when the
controlling process dies. The proxy experiments keep the turn busy with a per-run UserPromptSubmit
hook (`sleep N`, passed with `--settings`) instead of a Bash tool call.

Run it from a clean environment, as Pastey's own process would launch
`claude`: a shell inside a Claude Code session passes that session's host
variables (bare mode, its API base URL, host-delegated sign-in) to every
child `claude`.

    env -i HOME="$HOME" USER="$USER" PATH="$PATH" TMPDIR="$TMPDIR" python3 -I probe.py ...

The `cancel`, `orphan` and `exclusivity` experiments need one Bash command
to keep a turn running: they pass `--allowedTools "Bash(sleep N)"` for that
exact command, for that run only. No other permission is granted, and the
user's settings are never changed.
"""

import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid

SLEEP_SECONDS = 60


def claude():
    found = shutil.which("claude")
    if found:
        return found
    local = os.path.expanduser("~/.local/bin/claude")
    return local if os.path.isfile(local) else None


def workspace():
    return os.path.realpath(tempfile.mkdtemp(prefix="pastey-cc-probe-"))


def short(value):
    return (value or "")[:8]


def user_message(text):
    return {
        "type": "user",
        "message": {"role": "user", "content": text},
        "parent_tool_use_id": None,
        "session_id": "",
    }


def transcript_path(cwd, session_id):
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    encoded = "".join(c if c.isalnum() else "-" for c in cwd)
    return os.path.join(base, "projects", encoded, f"{session_id}.jsonl")


def transcript_summary(cwd, session_id):
    """Main-chain entry counts and how the transcript's last turn ended."""
    path = transcript_path(cwd, session_id)
    if not os.path.isfile(path):
        return {"exists": False}
    prompts = replies = 0
    last_stop = None
    last_kind = None
    session_ids = set()
    for line in open(path, encoding="utf-8", errors="replace"):
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if entry.get("sessionId"):
            session_ids.add(entry["sessionId"])
        if entry.get("isSidechain"):
            continue
        kind = entry.get("type")
        message = entry.get("message") or {}
        if kind == "user" and isinstance(message.get("content"), str):
            prompts += 1
        if kind == "assistant":
            replies += 1
            last_stop = message.get("stop_reason")
        if kind in ("user", "assistant"):
            last_kind = kind
    return {
        "exists": True,
        "promptEntries": prompts,
        "assistantEntries": replies,
        "lastKind": last_kind,
        "lastStopReason": last_stop,
        "sessionIds": sorted(short(s) for s in session_ids),
    }


def descendants(pid):
    """(pid, command) of every live descendant of `pid`."""
    try:
        table = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,command="], capture_output=True, text=True, timeout=10
        ).stdout
    except Exception:  # noqa: BLE001 - absence of evidence is reported as such
        return None
    children = {}
    commands = {}
    for row in table.splitlines():
        parts = row.split(None, 2)
        if len(parts) < 3:
            continue
        child, parent, command = int(parts[0]), int(parts[1]), parts[2]
        children.setdefault(parent, []).append(child)
        commands[child] = command
    found, stack = [], [pid]
    while stack:
        for child in children.get(stack.pop(), []):
            found.append((child, commands.get(child, "")[:60]))
            stack.append(child)
    return found


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def pids_running(pattern):
    """Live pids whose command line contains `pattern` (process-list fact)."""
    table = subprocess.run(
        ["ps", "-axo", "pid=,command="], capture_output=True, text=True, timeout=10
    ).stdout
    return [int(row.split(None, 1)[0]) for row in table.splitlines() if pattern in row]


def registry(cli, session_id):
    """`claude agents --json` entry for the session, or None if not listed."""
    try:
        out = subprocess.run(
            [cli, "agents", "--json"], capture_output=True, text=True, timeout=15, check=True
        ).stdout
        for entry in json.loads(out):
            if entry.get("sessionId") == session_id:
                return {k: entry.get(k) for k in ("kind", "status", "state", "waitingFor")}
        return None
    except Exception as error:  # noqa: BLE001
        return {"unknown": type(error).__name__}


class Run:
    """One `claude -p` stream-json process, its stdout read on a thread."""

    def __init__(self, cli, cwd, extra, label):
        self.label = label
        self.started = time.monotonic()
        self.child = subprocess.Popen(
            [cli, "-p", "--input-format", "stream-json", "--output-format", "stream-json",
             "--verbose", *extra],
            cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        self.events = queue.Queue()
        self.log = []
        self.stderr = []
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._read_err, daemon=True).start()

    def _read(self):
        for line in self.child.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                event = {"type": "non-json", "bytes": len(line)}
            self.log.append(event)
            self.events.put(event)
        self.events.put(None)

    def _read_err(self):
        for line in self.child.stderr:
            self.stderr.append(line.rstrip()[:200])

    def send(self, payload):
        self.child.stdin.write(json.dumps(payload) + "\n")
        self.child.stdin.flush()

    def close_stdin(self):
        try:
            self.child.stdin.close()
        except OSError:
            pass

    def elapsed(self):
        return round(time.monotonic() - self.started, 1)

    def wait_for(self, predicate, timeout):
        """First event matching `predicate`, or None on timeout or end."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = self.events.get(timeout=max(0.05, deadline - time.monotonic()))
            except queue.Empty:
                return None
            if event is None:
                return None
            note(self, event)
            if predicate(event):
                return event
        return None

    def drain(self, timeout):
        self.wait_for(lambda _: False, timeout)

    def exit_code(self, timeout):
        try:
            return self.child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None


def note(run, event):
    kind = event.get("type")
    sub = event.get("subtype")
    detail = ""
    if kind == "system" and sub == "init":
        detail = (f"session={short(event.get('session_id'))} mode={event.get('permissionMode')} "
                  f"version={event.get('claude_code_version')} "
                  f"capabilities={event.get('capabilities')} apiKeySource={event.get('apiKeySource')}")
    elif kind == "assistant":
        blocks = (event.get("message") or {}).get("content") or []
        detail = ",".join(
            b.get("type") + (f":{b.get('name')}" if b.get("type") == "tool_use" else "")
            for b in blocks if isinstance(b, dict))
    elif kind == "user":
        blocks = (event.get("message") or {}).get("content")
        if isinstance(blocks, list):
            detail = ",".join(
                b.get("type") + (":error" if b.get("is_error") else "")
                for b in blocks if isinstance(b, dict))
    elif kind == "result":
        detail = (f"subtype={sub} is_error={event.get('is_error')} "
                  f"session={short(event.get('session_id'))} stop={event.get('stop_reason')} "
                  f"turns={event.get('num_turns')} denials={len(event.get('permission_denials') or [])} "
                  f"result={(event.get('result') or '')[:80]!r}")
    elif kind in ("control_request", "control_response"):
        body = event.get("request") or event.get("response") or {}
        detail = json.dumps({k: body.get(k) for k in ("subtype", "tool_name", "request_id", "error")})
    elif kind == "system":
        detail = json.dumps({k: event.get(k) for k in ("tool_name", "error", "status", "message")
                             if event.get(k) is not None})[:160]
    print(f"  [{run.label} +{run.elapsed():>5}s] {kind}/{sub or '-'} {detail}")


def is_result(event):
    return event.get("type") == "result"


def start_turn(cli, cwd, prompt, label, extra=()):
    run = Run(cli, cwd, list(extra), label)
    run.send(user_message(prompt))
    return run


# --- experiments ----------------------------------------------------------


def detect(cli):
    version = subprocess.run([cli, "--version"], capture_output=True, text=True, timeout=15)
    print("executable:", cli)
    print("version:", version.stdout.strip(), "exit", version.returncode)
    held = subprocess.run([cli, "agents", "--json"], capture_output=True, text=True, timeout=15)
    try:
        entries = json.loads(held.stdout)
        kinds = {}
        for entry in entries:
            kinds[entry.get("kind")] = kinds.get(entry.get("kind"), 0) + 1
        print("agents --json: exit", held.returncode, "entries by kind", kinds)
    except ValueError:
        print("agents --json: unreadable, exit", held.returncode)


def lifecycle(cli):
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nrequested session {short(session)}")
    print("1. new session with a chosen id")
    run = start_turn(cli, cwd, "Reply with exactly the word ALPHA and nothing else.", "new",
                     ["--session-id", session])
    held_during = None
    result = run.wait_for(is_result, 180)
    held_during = registry(cli, session)
    run.close_stdin()
    print(f"  exit={run.exit_code(30)} registry-after-result={held_during}")
    print("  transcript:", transcript_summary(cwd, session))

    print("2. same id again as a new session (must refuse: no silent replacement)")
    again = start_turn(cli, cwd, "Reply with exactly the word BETA.", "dup-new", ["--session-id", session])
    again.close_stdin()
    again.drain(60)
    print(f"  exit={again.exit_code(30)} stderr={again.stderr[:2]}")

    print("3. resume exactly that session")
    resumed = start_turn(cli, cwd, "What single word did you reply with last time? Reply with that word only.",
                         "resume", ["--resume", session])
    resumed_result = resumed.wait_for(is_result, 180)
    resumed.close_stdin()
    print(f"  exit={resumed.exit_code(30)} same-session="
          f"{bool(resumed_result) and resumed_result.get('session_id') == session}")
    print("  transcript:", transcript_summary(cwd, session))

    print("4. resume a session id that does not exist")
    missing = start_turn(cli, cwd, "Reply OK.", "missing", ["--resume", str(uuid.uuid4())])
    missing.close_stdin()
    missing.drain(60)
    print(f"  exit={missing.exit_code(30)} stderr={missing.stderr[:2]} events={[e.get('type') for e in missing.log]}")
    return result, resumed_result


def outcomes(cli):
    cwd = workspace()
    print(f"workspace {cwd}")
    print("1. a turn cut short by its own limit (--max-turns 1, task needs a tool)")
    run = start_turn(cli, cwd, "List the files in the current directory using a tool, then summarise.",
                     "max-turns", ["--max-turns", "1"])
    run.wait_for(is_result, 180)
    run.close_stdin()
    print(f"  exit={run.exit_code(30)}")
    print("2. a launch the CLI refuses before any turn (invalid option value)")
    bad = start_turn(cli, cwd, "Reply OK.", "bad-flag", ["--permission-mode", "not-a-mode"])
    bad.close_stdin()
    bad.drain(30)
    print(f"  exit={bad.exit_code(30)} stderr={bad.stderr[:2]} events={[e.get('type') for e in bad.log]}")


def sleeping_turn(cli, cwd, session, label):
    command = f"sleep {SLEEP_SECONDS}"
    return start_turn(
        cli, cwd, BASH_PROMPT.format(command=command),
        label, ["--session-id", session, "--allowedTools", f"Bash({command})"])


def wait_for_sleep(run):
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        run.drain(1)
        tree = descendants(run.child.pid) or []
        sleeps = [pid for pid, command in tree if command.startswith(f"sleep {SLEEP_SECONDS}")
                  or f"sleep {SLEEP_SECONDS}" in command]
        if sleeps:
            return sleeps
        if run.child.poll() is not None:
            return []
    return []


def cancel(cli, method):
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nsession {short(session)} method {method}")
    run = sleeping_turn(cli, cwd, session, method)
    sleeps = wait_for_sleep(run)
    print(f"  tool process running under claude: {sleeps} after {run.elapsed()}s")
    if not sleeps:
        print("  no tool process appeared; nothing to cancel")
        return
    tree_before = descendants(run.child.pid)
    print(f"  claude pid {run.child.pid} descendants {len(tree_before or [])}")
    t0 = time.monotonic()
    if method == "interrupt":
        run.send({"type": "control_request", "request_id": "probe-interrupt-1",
                  "request": {"subtype": "interrupt"}})
        result = run.wait_for(is_result, 60)
        print(f"  result after {round(time.monotonic() - t0, 1)}s: "
              f"{result and result.get('subtype')} alive={run.child.poll() is None}")
        run.close_stdin()
    elif method == "sigint":
        run.child.send_signal(signal.SIGINT)
        run.wait_for(is_result, 60)
    elif method == "sigterm":
        run.child.send_signal(signal.SIGTERM)
        run.drain(15)
    elif method == "stdin-eof":
        run.close_stdin()
        run.drain(10)
    code = run.exit_code(60)
    time.sleep(1)
    survivors = [pid for pid in sleeps if alive(pid)]
    print(f"  claude exit={code} after {round(time.monotonic() - t0, 1)}s; "
          f"tool processes still alive: {survivors}")
    print("  transcript:", transcript_summary(cwd, session))
    print("  registry:", registry(cli, session))
    for pid in survivors:
        os.kill(pid, signal.SIGTERM)


# Claude Code's Bash tool refuses a foreground `sleep` (2.1.317), so the
# command runs as a background task, as the model chose by itself there.
BASH_PROMPT = ("Use the Bash tool with run_in_background set to true to run exactly this "
               "command and nothing else: {command}\n"
               "When it finishes, reply with exactly the word SLEPT.")

PARENT = r"""
import subprocess, sys, json, time
cli, cwd, session, command, prompt = sys.argv[1:6]
child = subprocess.Popen(
    [cli, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--session-id", session, "--allowedTools", "Bash(" + command + ")"],
    cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
child.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": prompt},
    "parent_tool_use_id": None, "session_id": ""}) + "\n")
child.stdin.flush()
print(child.pid, flush=True)
for line in child.stdout:
    pass
"""


def orphan(cli, mode="bare"):
    """Pastey crashes: its process dies with SIGKILL while a real Bash tool
    call of the turn runs."""
    cwd = workspace()
    session = str(uuid.uuid4())
    command = f"sleep {SLEEP_SECONDS}"
    print(f"workspace {cwd}\nsession {short(session)} ({mode})")
    if mode == "watchdog":
        parent, claude_pid = start_watched(
            cli, cwd, BASH_PROMPT.format(command=command),
            ["--session-id", session, "--allowedTools", f"Bash({command})"])
        if claude_pid:
            orphan_watch(cli, cwd, session, parent, claude_pid)
        return
    parent = subprocess.Popen([sys.executable, "-I", "-c", PARENT, cli, cwd, session, command,
                               BASH_PROMPT.format(command=command)],
                              stdout=subprocess.PIPE, text=True)
    orphan_watch(cli, cwd, session, parent, int(parent.stdout.readline()))


def exclusivity(cli):
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nsession {short(session)}")
    first = sleeping_turn(cli, cwd, session, "holder")
    if not wait_for_sleep(first):
        print("  holder never reached its tool")
        return
    print("  holder is mid-turn; registry:", registry(cli, session))
    print("1. a second client resumes the held session (-p --resume)")
    second = start_turn(cli, cwd, "Reply with exactly the word SECOND.", "resumer", ["--resume", session])
    second_result = second.wait_for(is_result, 180)
    second.close_stdin()
    print(f"  exit={second.exit_code(30)} stderr={second.stderr[:2]} "
          f"wrote-same-session={bool(second_result) and second_result.get('session_id') == session}")
    print("2. a second client starts a new session with the held id (--session-id)")
    third = start_turn(cli, cwd, "Reply OK.", "same-id", ["--session-id", session])
    third.close_stdin()
    third.drain(60)
    print(f"  exit={third.exit_code(30)} stderr={third.stderr[:2]}")
    print("  transcript while holder runs:", transcript_summary(cwd, session))
    first.wait_for(is_result, SLEEP_SECONDS + 120)
    first.close_stdin()
    print(f"  holder exit={first.exit_code(30)}")
    print("  transcript after both:", transcript_summary(cwd, session))


def branches(cwd, session_id):
    """Main-chain entries that share a parent: two writers forked history."""
    path = transcript_path(cwd, session_id)
    parents = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if entry.get("type") in ("user", "assistant") and not entry.get("isSidechain"):
            parents[entry.get("parentUuid")] = parents.get(entry.get("parentUuid"), 0) + 1
    return sum(1 for parent, count in parents.items() if parent and count > 1)


def held_resume(cli):
    """A live `-p` process holds the session; another client resumes it.
    Needs no model turn: an idle streaming process holds the session."""
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nsession {short(session)}")
    holder = start_turn(cli, cwd, "Reply with exactly the word ONE.", "holder", ["--session-id", session])
    holder.wait_for(is_result, 120)
    print("  holder idle, stdin open; registry:", registry(cli, session))
    second = start_turn(cli, cwd, "Reply with exactly the word TWO.", "resumer", ["--resume", session])
    result = second.wait_for(is_result, 120)
    second.close_stdin()
    print(f"  resumer exit={second.exit_code(30)} stderr={second.stderr[:2]} "
          f"wrote-same-session={bool(result) and result.get('session_id') == session}")
    holder.send(user_message("Reply with exactly the word THREE."))
    holder.wait_for(is_result, 120)
    holder.close_stdin()
    print(f"  holder exit={holder.exit_code(30)}")
    print("  transcript:", transcript_summary(cwd, session), "forks:", branches(cwd, session))


def hook_settings(seconds):
    """A per-run UserPromptSubmit hook that keeps the turn busy in a child
    process of claude: a stand-in for a running tool when no model turn
    can run. Nothing persists; the user's settings are untouched."""
    return json.dumps({"hooks": {"UserPromptSubmit": [{"hooks": [
        {"type": "command", "command": f"sleep {seconds}", "timeout": seconds + 60}]}]}})


def proxy_cancel(cli, method):
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nsession {short(session)} method {method} (hook stand-in)")
    run = start_turn(cli, cwd, "Reply OK.", method,
                     ["--session-id", session, "--settings", hook_settings(SLEEP_SECONDS)])
    sleeps = wait_for_sleep(run)
    print(f"  hook process under claude: {sleeps} after {run.elapsed()}s; "
          f"registry: {registry(cli, session)}")
    if not sleeps:
        print(f"  no hook process appeared; stderr={run.stderr[:3]}")
        run.child.kill()
        return
    t0 = time.monotonic()
    if method == "interrupt":
        run.send({"type": "control_request", "request_id": "probe-interrupt-1",
                  "request": {"subtype": "interrupt"}})
        run.wait_for(is_result, 30)
        print(f"  after interrupt {round(time.monotonic() - t0, 1)}s: claude alive={run.child.poll() is None} "
              f"hook alive={[p for p in sleeps if alive(p)]}")
        run.close_stdin()
    elif method == "sigint":
        run.child.send_signal(signal.SIGINT)
        run.wait_for(is_result, 30)
    elif method == "sigterm":
        run.child.send_signal(signal.SIGTERM)
        run.drain(10)
    elif method == "stdin-eof":
        run.close_stdin()
        run.drain(10)
    code = run.exit_code(SLEEP_SECONDS + 60)
    time.sleep(1)
    survivors = [pid for pid in sleeps if alive(pid)]
    print(f"  claude exit={code} after {round(time.monotonic() - t0, 1)}s; "
          f"hook processes still alive: {survivors}")
    print("  transcript:", transcript_summary(cwd, session))
    for pid in survivors:
        os.kill(pid, signal.SIGTERM)
        print(f"  cleanup: sent SIGTERM to leftover pid {pid}")


PROXY_PARENT = r"""
import subprocess, sys, json
cli, cwd, session, settings = sys.argv[1:5]
child = subprocess.Popen(
    [cli, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--session-id", session, "--settings", settings],
    cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
child.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": "Reply OK."},
    "parent_tool_use_id": None, "session_id": ""}) + "\n")
child.stdin.flush()
print(child.pid, flush=True)
for line in child.stdout:
    pass
"""


# Stand-in for an adapter-owned lifeline: claude runs as this watchdog's
# child in its own process group, with stdin/stdout passed straight through
# to Pastey. Pastey never writes the lifeline (fd 3); it reads EOF only once
# every Pastey copy of the write end is gone, that is, when Pastey died.
# Then the watchdog ends the turn as Stop would (SIGINT), and escalates.
WATCHDOG = r"""
import os, signal, subprocess, sys, threading, time
child = subprocess.Popen(sys.argv[1:], start_new_session=True)
def lifeline():
    while os.read(3, 1):
        pass
    for sig, grace in ((signal.SIGINT, 10), (signal.SIGTERM, 5), (signal.SIGKILL, 0)):
        if child.poll() is not None:
            break
        os.killpg(child.pid, sig)
        try:
            child.wait(timeout=grace or None)
        except subprocess.TimeoutExpired:
            pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
threading.Thread(target=lifeline, daemon=True).start()
sys.exit(child.wait())
"""

WATCHED_PARENT = r"""
import os, subprocess, sys, json
watchdog, cli, cwd, prompt, extra = sys.argv[1:6]
read_end, write_end = os.pipe()
os.dup2(read_end, 3) if read_end != 3 else None
child = subprocess.Popen(
    [sys.executable, "-I", "-c", watchdog, cli, "-p", "--input-format", "stream-json",
     "--output-format", "stream-json", "--verbose", *json.loads(extra)],
    cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    pass_fds=(3,))
os.close(read_end) if read_end != 3 else None
os.close(3)
child.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": prompt},
    "parent_tool_use_id": None, "session_id": ""}) + "\n")
child.stdin.flush()
print(child.pid, flush=True)
for line in child.stdout:
    pass
"""


def start_watched(cli, cwd, prompt, extra):
    """A stand-in Pastey that runs claude under the lifeline watchdog:
    (the stand-in, claude's pid), or claude's pid None if it never started."""
    parent = subprocess.Popen(
        [sys.executable, "-I", "-c", WATCHED_PARENT, WATCHDOG, cli, cwd, prompt, json.dumps(extra)],
        stdout=subprocess.PIPE, text=True)
    watchdog_pid = int(parent.stdout.readline())
    deadline = time.monotonic() + 30
    tree = []
    while time.monotonic() < deadline and not tree:
        time.sleep(0.2)
        tree = [pid for pid, c in (descendants(watchdog_pid) or []) if " -p " in f" {c} "
                or c.endswith("claude") or "/claude" in c.split(" ")[0]]
    if not tree:
        print("  watchdog started no claude; stopping")
        parent.kill()
        return parent, None
    print(f"  watchdog pid {watchdog_pid}")
    return parent, tree[0]


def proxy_orphan(cli, mode="bare"):
    """The controlling process dies (SIGKILL) while claude's child runs."""
    cwd = workspace()
    session = str(uuid.uuid4())
    print(f"workspace {cwd}\nsession {short(session)} (hook stand-in, {mode})")
    if mode == "watchdog":
        parent, claude_pid = start_watched(
            cli, cwd, "Reply OK.",
            ["--session-id", session, "--settings", hook_settings(SLEEP_SECONDS)])
        if claude_pid:
            orphan_watch(cli, cwd, session, parent, claude_pid)
        return
    parent = subprocess.Popen(
        [sys.executable, "-I", "-c", PROXY_PARENT, cli, cwd, session, hook_settings(SLEEP_SECONDS)],
        stdout=subprocess.PIPE, text=True)
    orphan_watch(cli, cwd, session, parent, int(parent.stdout.readline()))


def orphan_watch(cli, cwd, session, parent, claude_pid):
    print(f"  stand-in Pastey pid {parent.pid}, its claude pid {claude_pid}")
    deadline = time.monotonic() + 120
    sleeps = []
    while time.monotonic() < deadline and not sleeps:
        time.sleep(0.5)
        sleeps = [pid for pid, c in (descendants(claude_pid) or []) if f"sleep {SLEEP_SECONDS}" in c]
    if not sleeps:
        print("  no child process appeared; stopping")
        parent.kill()
        return
    print(f"  child {sleeps} running; killing the stand-in Pastey with SIGKILL")
    killed_at = time.monotonic()
    parent.kill()
    parent.wait()
    for second in (1, 5, 15, 30, SLEEP_SECONDS + 15, SLEEP_SECONDS + 45):
        time.sleep(max(0, killed_at + second - time.monotonic()))
        print(f"  +{second:>3}s claude alive={alive(claude_pid)} "
              f"child alive={[p for p in sleeps if alive(p)]} "
              f"registry={registry(cli, session)} transcript={transcript_summary(cwd, session)}")
        if not alive(claude_pid) and not any(alive(p) for p in sleeps):
            break
    for pid in [claude_pid, *sleeps]:
        if alive(pid):
            os.kill(pid, signal.SIGTERM)
            print(f"  cleanup: sent SIGTERM to leftover pid {pid}")


def untrusted_folder():
    """A new folder whose project configuration would run commands: two
    hooks and one MCP server, each only touching a marker file in it."""
    cwd = workspace()
    os.mkdir(os.path.join(cwd, ".claude"))
    with open(os.path.join(cwd, ".claude", "settings.json"), "w") as settings:
        json.dump({"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "touch hook-sessionstart-ran"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "touch hook-userprompt-ran"}]}],
        }}, settings)
    with open(os.path.join(cwd, ".mcp.json"), "w") as mcp:
        json.dump({"mcpServers": {"trustprobe": {
            "command": "/bin/sh", "args": ["-c", "touch mcp-server-ran; sleep 3"]}}}, mcp)
    return cwd


def markers(cwd):
    return sorted(name for name in os.listdir(cwd) if name.endswith("-ran"))


def trust(cli):
    """What each launch runs in a never-trusted folder, before any model."""
    cwd = untrusted_folder()
    print(f"1. claude -p in {cwd}")
    run = start_turn(cli, cwd, "Reply OK.", "print")
    first = run.wait_for(lambda e: e.get("type") in ("system", "result"), 60)
    run.close_stdin()
    run.drain(30)
    print(f"  exit={run.exit_code(30)} first event={first and (first.get('type'), first.get('subtype'))} "
          f"ran={markers(cwd)}")
    cwd = untrusted_folder()
    session = str(uuid.uuid4())
    print(f"2. claude --bg in {cwd}")
    launched = subprocess.run([cli, "--bg", "--session-id", session, "Reply OK."], cwd=cwd,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
    time.sleep(4)
    print(f"  exit={launched.returncode} said={(launched.stdout + launched.stderr).strip()[:60]!r} "
          f"ran={markers(cwd)} registry={registry(cli, session)}")
    if registry(cli, session):
        subprocess.run([cli, "stop", session[:8]], capture_output=True, timeout=30)
        print("  cleanup: stopped the background session")


def permission(cli, mode):
    """A tool call that needs approval in a default-mode headless run."""
    cwd = workspace()
    session = str(uuid.uuid4())
    extra = ["--session-id", session]
    if mode == "stdio-host":
        # The Agent SDK's own wiring when its host answers prompts.
        extra += ["--permission-prompt-tool", "stdio"]
    print(f"workspace {cwd}\nsession {short(session)} mode {mode}")
    run = start_turn(cli, cwd,
                     "Create a file named probe.txt containing the word hi, using the Write tool. "
                     "If you cannot, reply with exactly the word REFUSED.", mode, extra)
    request = run.wait_for(lambda e: e.get("type") == "control_request" or is_result(e), 180)
    if request and request.get("type") == "control_request":
        body = request.get("request") or {}
        print(f"  host was asked: {body.get('subtype')} for {body.get('tool_name')}; denying it")
        run.send({"type": "control_response", "response": {
            "subtype": "success", "request_id": request.get("request_id"),
            "response": {"behavior": "deny", "message": "Denied by the probe."}}})
        run.wait_for(is_result, 180)
    run.close_stdin()
    print(f"  exit={run.exit_code(30)} file-created={os.path.exists(os.path.join(cwd, 'probe.txt'))}")


def main():
    cli = claude()
    if not cli:
        sys.exit("No `claude` executable was found.")
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    experiment, args = sys.argv[1], sys.argv[2:]
    {"detect": detect, "lifecycle": lifecycle, "outcomes": outcomes,
     "exclusivity": exclusivity, "held-resume": held_resume,
     "trust": trust}.get(experiment, lambda _: None)(cli)
    if experiment == "orphan":
        orphan(cli, args[0] if args else "bare")
    if experiment == "proxy-orphan":
        proxy_orphan(cli, args[0] if args else "bare")
    if experiment == "cancel":
        cancel(cli, args[0])
    if experiment == "proxy-cancel":
        proxy_cancel(cli, args[0])
    if experiment == "permission":
        permission(cli, args[0])


if __name__ == "__main__":
    main()
