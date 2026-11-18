"""M0 measurements for Collaboration Links
(docs/research/agent-collaboration-links.md).

Research only; nothing here is part of the Pastey app. It drives the user's
own installed `codex` and `claude` as Pastey's adapters do: `codex
app-server --stdio` with `thread/start {cwd}` and `turn/start`, and `claude
-p` with stream-json in and out. Pastey chooses no model, permission mode or
settings. The only exceptions are the per-run flags an experiment exists to
measure, each named below. Every run happens in a new temporary folder.
Children get a clean environment (HOME, USER, PATH, TMPDIR and locale only),
so a shell inside a Claude Code session does not pass its own host variables
to them.

    python3 -I probe.py codex-mcp [MODE ...]
        Codex app-server with a per-process MCP server (`-c mcp_servers.*`),
        one turn per `default_tools_approval_mode` (default: unset, auto,
        prompt, writes, approve). Records any request Codex sends the client,
        the tool call's status, the spool, and whether ~/.codex/config.toml
        changed.
    python3 -I probe.py claude-mcp
        A new `claude -p` session, then two `--resume` runs with
        `--mcp-config`: one with `--allowedTools mcp__pastey__send`, one
        without. Records the session id, the transcripts in the project
        folder, permission denials, the spool, and whether the user's
        settings changed.
    python3 -I probe.py claude-inbound [refuse]
        A `claude -p` turn stays busy in one foreground tool call
        (`--allowedTools "Bash(python3 -c *)"`) while a second `claude -p`
        session sends it one message with SendMessage (`--allowedTools
        SendMessage ListAgents`). `refuse` starts the receiver with
        `--settings '{"crossSessionInbound":"refuse"}'`.
    python3 -I probe.py trailer codex|claude warranted|unwarranted N [--jobs J] [--out DIR]
        N independent turns with the link note Pastey would add. `warranted`:
        the task finds something the linked peer must change. `unwarranted`:
        nothing concerns the peer. Each final reply is checked against the
        design's block rules. Claude runs use --no-session-persistence and
        Codex threads are ephemeral, so neither adds to the user's history.

Model turns use the user's quota. Reply text is written only to the output
folder (default: a new temporary folder), never printed.
"""

import concurrent.futures
import hashlib
import json
import os
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
STUB = os.path.join(HERE, "mcp_stub.py")
TURN_TIMEOUT = 300
MAX_BODY_BYTES = 8 * 1024
MAX_SUBJECT_CHARS = 200
MAX_BLOCKS = 3
V2_APPROVALS = {
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
}


# --- environment and helpers ----------------------------------------------

def clean_env():
    keep = ("HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "SHELL", "LANG", "LC_ALL")
    return {name: os.environ[name] for name in keep if name in os.environ}


def cli(name):
    found = shutil.which(name)
    if found:
        return found
    local = os.path.expanduser(f"~/.local/bin/{name}")
    return local if os.path.isfile(local) else None


def folder(prefix):
    return os.path.realpath(tempfile.mkdtemp(prefix=f"pastey-collab-{prefix}-"))


def digest(path):
    try:
        with open(path, "rb") as handle:
            return hashlib.blake2b(handle.read(), digest_size=8).hexdigest()
    except OSError:
        return None


def read_spool(path):
    if not os.path.isfile(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def stub_command(spool):
    return [cli("python3") or sys.executable, "-I", STUB, spool]


def show(label, facts):
    print(f"{label}: {json.dumps(facts, sort_keys=True)}")


# --- the fixture and the link note ------------------------------------------

FIXTURE = {
    "README.md": (
        "# Shop\n\n"
        "Two parts: `api/` (the HTTP API) and `web/` (the browser client).\n"
        "`web/` is maintained by a different Agent session.\n"
    ),
    "api/orders.py": (
        '"""Orders API."""\n\n'
        "# 2026-11-18: /v2/orders switched from offset to cursor pagination.\n\n\n"
        "def list_orders(cursor=None, limit=50):\n"
        '    """GET /v2/orders?cursor=<opaque>&limit=<n>\n\n'
        '    Returns {"orders": [...], "nextCursor": <opaque or None>}.\n'
        "    The `offset` parameter was removed; a request that sends it is\n"
        "    rejected with 400.\n"
        '    """\n'
        "    raise NotImplementedError\n"
    ),
    "api/health.py": (
        "VERSION = \"1.4.2\"\n\n\n"
        "def health():\n"
        '    """GET /health"""\n'
        '    return {"status": "ok", "version": VERSION}\n'
    ),
    "web/OrdersTable.js": (
        "// The orders table in the web client.\n"
        "export async function loadPage(page, pageSize = 50) {\n"
        "  const offset = page * pageSize;\n"
        "  const response = await fetch(`/v2/orders?offset=${offset}&limit=${pageSize}`);\n"
        "  const body = await response.json();\n"
        "  return { rows: body.orders, hasMore: body.orders.length === pageSize };\n"
        "}\n"
    ),
}

TASKS = {
    "warranted": (
        "Review the pagination change described in api/orders.py and check whether "
        "anything else in this repository still depends on the old behavior. Do not "
        "modify any files; report what you found."
    ),
    "unwarranted": (
        "Explain in two sentences what GET /health in api/health.py returns. "
        "Do not modify any files."
    ),
}


def link_note(peer_agent):
    return (
        "[Pastey: this session is linked with another Agent session]\n"
        f'Linked peer: "web", a {peer_agent} session that maintains web/ in this '
        "repository on another device. You cannot edit web/ yourself.\n"
        "If, during this task, you find something that peer must know or do, end "
        "your final answer with a block in exactly this form (at most 3 blocks):\n\n"
        "```pastey-message to=web kind=request\n"
        "subject: <one line>\n\n"
        "<what the peer needs to know or do>\n"
        "```\n\n"
        "Use kind=request when the peer should act and kind=info when no action is "
        "needed. Send nothing if the peer does not need to know anything.\n"
        "[End of Pastey note]\n\n"
    )


def make_fixture(prefix):
    root = folder(prefix)
    for relative, text in FIXTURE.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    return root


# --- the block rules ----------------------------------------------------------

BLOCK = re.compile(r"^```pastey-message([^\n]*)\n(.*?)\n```[ \t]*$", re.S | re.M)
INVISIBLE = set(
    "­͏؜܏᠎​‌‍‎‏"
    "  ‪‫‬‭‮⁠⁡⁢⁣⁤"
    "⁦⁧⁨⁩ㅤ﻿ﾠ"
)


def renders_faithfully(text):
    """The rule of `renders_faithfully` in capability/codex.rs, approximately."""
    for character in text:
        if character in "\n\r\t":
            continue
        if ord(character) < 0x20 or 0x7F <= ord(character) < 0xA0:
            return False
        if character in INVISIBLE:
            return False
    return True


def check_blocks(text):
    """Facts about the pastey-message blocks in one final reply."""
    matches = list(BLOCK.finditer(text or ""))
    facts = {"blocks": len(matches), "valid": 0, "atEnd": False, "problems": []}
    if not matches:
        return facts
    tail = text[matches[0].start():]
    facts["atEnd"] = BLOCK.sub("", tail).strip() == ""
    for match in matches:
        attrs = dict(part.split("=", 1) for part in match.group(1).split() if "=" in part)
        body = match.group(2)
        lines = body.split("\n")
        subject = lines[0][len("subject:"):].strip() if lines[0].startswith("subject:") else None
        problems = []
        if attrs.get("to") != "web":
            problems.append(f"to={attrs.get('to')}")
        if attrs.get("kind") not in ("request", "info"):
            problems.append(f"kind={attrs.get('kind')}")
        if not subject or len(subject) > MAX_SUBJECT_CHARS:
            problems.append("subject")
        if len(body.encode("utf-8")) > MAX_BODY_BYTES:
            problems.append("body-size")
        if not renders_faithfully(body):
            problems.append("not-faithful")
        if problems:
            facts["problems"].append(problems)
        else:
            facts["valid"] += 1
        facts.setdefault("kinds", []).append(attrs.get("kind"))
    facts["accepted"] = (
        facts["atEnd"] and facts["valid"] == len(matches) and len(matches) <= MAX_BLOCKS
    )
    return facts


# --- Claude Code ----------------------------------------------------------------

def user_message(text):
    return {
        "type": "user",
        "message": {"role": "user", "content": text},
        "parent_tool_use_id": None,
        "session_id": "",
    }


class ClaudeRun:
    """One `claude -p` stream-json process."""

    def __init__(self, cwd, extra, label):
        self.label = label
        self.started = time.monotonic()
        self.child = subprocess.Popen(
            [cli("claude"), "-p", "--input-format", "stream-json",
             "--output-format", "stream-json", "--verbose", *extra],
            cwd=cwd, env=clean_env(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1,
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
                event = {"type": "non-json"}
            event["_at"] = round(time.monotonic() - self.started, 1)
            self.log.append(event)
            self.events.put(event)
        self.events.put(None)

    def _read_err(self):
        for line in self.child.stderr:
            self.stderr.append(line.rstrip()[:200])

    def prompt(self, text):
        self.child.stdin.write(json.dumps(user_message(text)) + "\n")
        self.child.stdin.flush()

    def wait_for(self, predicate, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = self.events.get(timeout=max(0.05, deadline - time.monotonic()))
            except queue.Empty:
                return None
            if event is None:
                return None
            if predicate(event):
                return event
        return None

    def finish(self, timeout=TURN_TIMEOUT):
        """The `result` event, then the exit code (stdin closed after it)."""
        result = self.wait_for(lambda e: e.get("type") == "result", timeout)
        try:
            self.child.stdin.close()
        except OSError:
            pass
        try:
            code = self.child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.child.kill()
            code = None
        return result, code

    def init(self):
        return next((e for e in self.log if e.get("type") == "system"
                     and e.get("subtype") == "init"), {})


def claude_project_dir(cwd):
    base = os.path.expanduser("~/.claude")
    encoded = "".join(c if c.isalnum() else "-" for c in cwd)
    return os.path.join(base, "projects", encoded)


def claude_settings_digests():
    base = os.path.expanduser("~/.claude")
    return {name: digest(os.path.join(base, name)) for name in ("settings.json", "settings.local.json")}


def result_facts(result, code):
    if not result:
        return {"result": None, "exit": code}
    return {
        "subtype": result.get("subtype"),
        "isError": result.get("is_error"),
        "session": (result.get("session_id") or "")[:8],
        "denials": [d.get("tool_name") for d in result.get("permission_denials") or []],
        "exit": code,
    }


def claude_mcp():
    cwd = folder("claude-mcp")
    spool = os.path.join(cwd, ".spool.jsonl")
    config = os.path.join(folder("claude-mcp-config"), "mcp.json")
    with open(config, "w", encoding="utf-8") as handle:
        command = stub_command(spool)
        json.dump({"mcpServers": {"pastey": {
            "type": "stdio", "command": command[0], "args": command[1:]}}}, handle)
    os.chmod(config, 0o600)
    settings_before = claude_settings_digests()
    session = str(uuid.uuid4())

    first = ClaudeRun(cwd, ["--session-id", session], "new")
    first.prompt("Reply with the single word OK.")
    show("new session", result_facts(*first.finish()))

    for label, extra in (
        ("resume+mcp+allowed", ["--allowedTools", "mcp__pastey__send"]),
        ("resume+mcp", []),
    ):
        token = f"m0-{label}"
        run = ClaudeRun(cwd, ["--resume", session, "--mcp-config", config, *extra], label)
        run.prompt(
            "Call the `send` tool of the `pastey` MCP server exactly once, with "
            f"to=web, kind=info and text={token}. Then reply DONE."
        )
        result, code = run.finish()
        init = run.init()
        facts = result_facts(result, code)
        facts["initSession"] = (init.get("session_id") or "")[:8]
        facts["mcpServers"] = [
            {k: s.get(k) for k in ("name", "status")} for s in init.get("mcp_servers") or []
        ]
        facts["spoolCalls"] = [
            c["arguments"].get("text") for c in read_spool(spool) if c.get("event") == "call"
        ]
        show(label, facts)

    project = claude_project_dir(cwd)
    transcripts = sorted(name for name in os.listdir(project) if name.endswith(".jsonl")) \
        if os.path.isdir(project) else []
    show("transcripts", {"expected": f"{session[:8]}.jsonl",
                         "found": [name[:8] + ".jsonl" for name in transcripts]})
    started = [e for e in read_spool(spool) if e.get("event") == "started"]
    show("stub environment", {"envNames": sorted({n for e in started for n in e["envNames"]})})
    show("user settings unchanged", {"unchanged": claude_settings_digests() == settings_before})


def claude_inbound(refuse):
    receiver_cwd = folder("claude-recv")
    sender_cwd = folder("claude-send")
    name = f"m0-recv-{uuid.uuid4().hex[:6]}"
    token = f"m0-ping-{uuid.uuid4().hex[:8]}"
    # Claude Code blocks a standalone `sleep` and moves it to the background
    # (measured, 2.1.318), which ends the turn early. A foreground Python
    # sleep keeps the turn running inside one tool call.
    busy = 'python3 -c "import time; time.sleep(40)"'
    extra = ["--name", name, "--allowedTools", "Bash(python3 -c *)"]
    if refuse:
        extra += ["--settings", json.dumps({"crossSessionInbound": "refuse"})]
    receiver = ClaudeRun(receiver_cwd, extra, "recv")
    receiver.prompt(
        f"Run exactly this shell command with the Bash tool, in the foreground (do not "
        f"run it in the background): {busy}\nAfter it finishes, reply with every "
        "message you received from another Claude Code session during this turn, "
        "quoted exactly, or reply NONE."
    )
    sleeping = receiver.wait_for(
        lambda e: e.get("type") == "assistant" and any(
            b.get("type") == "tool_use" for b in (e.get("message") or {}).get("content") or []
            if isinstance(b, dict)),
        120,
    )
    show("receiver sleeping", {"seen": bool(sleeping), "at": sleeping and sleeping["_at"]})
    time.sleep(3)

    sender = ClaudeRun(sender_cwd, ["--allowedTools", "SendMessage", "ListAgents"], "send")
    sender.prompt(
        "Send one cross-session message with the SendMessage tool to the Claude Code "
        f"session named {name}. The message text must be exactly: {token}\n"
        "Do not message any other session, subagent or teammate. Then reply with the "
        "tool's result in one line."
    )
    sent_result, sent_code = sender.finish(180)
    facts = result_facts(sent_result, sent_code)
    facts["reply"] = ((sent_result or {}).get("result") or "")[:200]
    show("sender", facts)

    result, code = receiver.finish(180)
    # Every event up to the process's end, in order: is the message between
    # the busy tool call's result and the turn's first `result`?
    receiver.wait_for(lambda _: False, 5)
    timeline = []
    for event in receiver.log:
        kind = event.get("type")
        blocks = (event.get("message") or {}).get("content")
        blocks = blocks if isinstance(blocks, list) else []
        if kind == "assistant" and any(b.get("type") == "tool_use" for b in blocks
                                       if isinstance(b, dict)):
            timeline.append(("tool_use", event["_at"]))
        elif kind == "user" and any(b.get("type") == "tool_result" for b in blocks
                                    if isinstance(b, dict)):
            timeline.append(("tool_result", event["_at"]))
        if token in json.dumps(event):
            timeline.append((f"token-in-{kind}", event["_at"]))
        if kind == "result":
            timeline.append(("result", event["_at"]))
    facts = result_facts(result, code)
    facts["timeline"] = timeline
    facts["tokenInFirstReply"] = token in ((result or {}).get("result") or "")
    facts["results"] = sum(1 for e in receiver.log if e.get("type") == "result")
    show("receiver", facts)
    show("receiver session", {"id": (result or {}).get("session_id")})


def claude_trailer(variant, out, index):
    cwd = make_fixture(f"claude-{variant}")
    run = ClaudeRun(cwd, ["--session-id", str(uuid.uuid4()), "--no-session-persistence"],
                    f"{variant}-{index}")
    run.prompt(link_note("Codex") + TASKS[variant])
    result, code = run.finish()
    text = (result or {}).get("result") or ""
    with open(os.path.join(out, f"claude-{variant}-{index:02}.md"), "w", encoding="utf-8") as h:
        h.write(text)
    facts = check_blocks(text)
    facts.update({
        "provider": "claude", "variant": variant, "run": index,
        "model": run.init().get("model"), "seconds": round(time.monotonic() - run.started),
        "outcome": (result or {}).get("subtype"), "exit": code,
        "denials": len((result or {}).get("permission_denials") or []),
    })
    shutil.rmtree(cwd, ignore_errors=True)
    return facts


# --- Codex ------------------------------------------------------------------------

class Codex:
    """One `codex app-server --stdio` process, as Pastey launches it."""

    def __init__(self, cwd, overrides=()):
        args = [cli("codex"), "app-server"]
        for override in overrides:
            args += ["-c", override]
        args.append("--stdio")
        self.child = subprocess.Popen(
            args, cwd=cwd, env=clean_env(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )
        self.incoming = queue.Queue()
        self.backlog = []
        self.next_id = 100
        threading.Thread(target=self._read, daemon=True).start()
        result = self.request("initialize", {
            "clientInfo": {"name": "Pastey", "version": "m0-probe"}, "capabilities": {}})
        self.user_agent = (result or {}).get("userAgent")
        self.send({"method": "initialized", "params": {}})

    def _read(self):
        for line in self.child.stdout:
            try:
                self.incoming.put(json.loads(line))
            except ValueError:
                continue
        self.incoming.put(None)

    def send(self, payload):
        self.child.stdin.write(json.dumps(payload) + "\n")
        self.child.stdin.flush()

    def request(self, method, params, timeout=60):
        self.next_id += 1
        wanted = self.next_id
        self.send({"id": wanted, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                message = self.incoming.get(timeout=max(0.05, deadline - time.monotonic()))
            except queue.Empty:
                break
            if message is None:
                break
            if message.get("id") == wanted and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result")
            self.backlog.append(message)
        raise RuntimeError(f"{method}: no answer")

    def next_message(self, timeout):
        if self.backlog:
            return self.backlog.pop(0)
        try:
            return self.incoming.get(timeout=timeout)
        except queue.Empty:
            return {}

    def run_turn(self, cwd, prompt):
        """One thread and one turn. Never answers a request except to decline
        a command or file change; anything else interrupts the turn."""
        thread = self.request("thread/start", {"cwd": cwd, "ephemeral": True})
        thread_id = thread["thread"]["id"]
        turn = self.request("turn/start", {
            "threadId": thread_id, "input": [{"type": "text", "text": prompt}]})
        turn_id = turn["turn"]["id"]
        report = {"model": thread.get("model"), "requests": [], "toolCalls": [],
                  "messages": [], "status": None}
        interrupted = False
        deadline = time.monotonic() + TURN_TIMEOUT
        while time.monotonic() < deadline:
            message = self.next_message(1)
            if message is None:
                report["status"] = "app-server-ended"
                break
            method = message.get("method")
            if method and "id" in message:
                report["requests"].append(method)
                if method in V2_APPROVALS:
                    self.send({"id": message["id"], "result": {"decision": "decline"}})
                elif not interrupted:
                    interrupted = True
                    self.request("turn/interrupt", {"threadId": thread_id, "turnId": turn_id})
            elif method == "item/completed":
                item = (message.get("params") or {}).get("item") or {}
                if item.get("type") == "mcpToolCall":
                    report["toolCalls"].append({
                        "server": item.get("server"), "tool": item.get("tool"),
                        "status": item.get("status"), "error": item.get("error")})
                elif item.get("type") == "agentMessage":
                    report["messages"].append({"phase": item.get("phase"),
                                               "text": item.get("text") or ""})
            elif method == "turn/completed":
                report["status"] = ((message.get("params") or {}).get("turn") or {}).get("status")
                break
        else:
            report["status"] = "probe-timeout"
            self.request("turn/interrupt", {"threadId": thread_id, "turnId": turn_id})
        return report

    def close(self):
        try:
            self.child.stdin.close()
            self.child.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            self.child.kill()


def final_text(report):
    finals = [m["text"] for m in report["messages"] if m["phase"] == "final_answer"]
    if finals:
        return finals[-1]
    return report["messages"][-1]["text"] if report["messages"] else ""


def toml_string(value):
    return json.dumps(value)


def codex_mcp(modes):
    config_toml = os.path.expanduser("~/.codex/config.toml")
    before = digest(config_toml)
    for mode in modes:
        cwd = folder(f"codex-mcp-{mode}")
        spool = os.path.join(cwd, ".spool.jsonl")
        command = stub_command(spool)
        overrides = [
            f"mcp_servers.pastey.command={toml_string(command[0])}",
            "mcp_servers.pastey.args=[" + ",".join(toml_string(a) for a in command[1:]) + "]",
        ]
        if mode != "unset":
            overrides.append(f"mcp_servers.pastey.default_tools_approval_mode={toml_string(mode)}")
        token = f"m0-codex-{mode}"
        codex = Codex(cwd, overrides)
        try:
            report = codex.run_turn(cwd, (
                "Call the `send` tool of the `pastey` MCP server exactly once, with "
                f"to=web, kind=info and text={token}. Then reply DONE."))
        finally:
            codex.close()
        show(f"mode={mode}", {
            "codex": codex.user_agent, "status": report["status"],
            "requests": report["requests"], "toolCalls": report["toolCalls"],
            "spoolCalls": [c["arguments"].get("text") for c in read_spool(spool)
                           if c.get("event") == "call"],
            "stubStarted": any(e.get("event") == "started" for e in read_spool(spool)),
        })
    show("config.toml unchanged", {"unchanged": digest(config_toml) == before})


def codex_trailer(variant, out, index):
    cwd = make_fixture(f"codex-{variant}")
    started = time.monotonic()
    codex = Codex(cwd)
    try:
        report = codex.run_turn(cwd, link_note("Claude Code") + TASKS[variant])
    finally:
        codex.close()
    text = final_text(report)
    with open(os.path.join(out, f"codex-{variant}-{index:02}.md"), "w", encoding="utf-8") as h:
        h.write(text)
    facts = check_blocks(text)
    facts.update({
        "provider": "codex", "variant": variant, "run": index, "model": report["model"],
        "seconds": round(time.monotonic() - started), "outcome": report["status"],
        "requests": report["requests"],
    })
    shutil.rmtree(cwd, ignore_errors=True)
    return facts


# --- trailer runs ----------------------------------------------------------------

def trailer(provider, variant, runs, jobs, out):
    os.makedirs(out, exist_ok=True)
    one = claude_trailer if provider == "claude" else codex_trailer
    results_path = os.path.join(out, f"{provider}-{variant}.jsonl")
    print(f"output: {out}")
    lock = threading.Lock()
    collected = []

    def task(index):
        try:
            facts = one(variant, out, index)
        except Exception as error:  # a probe failure, not an Agent result
            facts = {"provider": provider, "variant": variant, "run": index,
                     "probeError": str(error)[:200]}
        with lock:
            collected.append(facts)
            with open(results_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(facts) + "\n")
            show(f"{provider} {variant} #{index:02}", {
                k: facts.get(k) for k in ("blocks", "accepted", "kinds", "problems",
                                          "atEnd", "outcome", "seconds", "probeError")})

    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        list(pool.map(task, range(1, runs + 1)))

    completed = [f for f in collected if "probeError" not in f]
    show(f"summary {provider} {variant}", {
        "runs": len(collected),
        "probeErrors": len(collected) - len(completed),
        "withBlock": sum(1 for f in completed if f.get("blocks")),
        "accepted": sum(1 for f in completed if f.get("accepted")),
        "kinds": sorted({k for f in completed for k in f.get("kinds") or [] if k}),
        "models": sorted({f.get("model") or "?" for f in completed}),
    })


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    experiment = args[0]
    if experiment == "codex-mcp":
        codex_mcp(args[1:] or ["unset", "auto", "prompt", "writes", "approve"])
    elif experiment == "claude-mcp":
        claude_mcp()
    elif experiment == "claude-inbound":
        claude_inbound(refuse=args[1:2] == ["refuse"])
    elif experiment == "trailer":
        provider, variant, runs = args[1], args[2], int(args[3])
        jobs = int(args[args.index("--jobs") + 1]) if "--jobs" in args else 1
        out = args[args.index("--out") + 1] if "--out" in args else folder("results")
        if provider not in ("codex", "claude") or variant not in TASKS:
            print(__doc__)
            return 2
        trailer(provider, variant, runs, jobs, out)
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
