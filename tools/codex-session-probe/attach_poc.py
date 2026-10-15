"""Minimal D2 PoC: two clients of one Codex app-server.

It starts its own app-server on a private Unix socket in a new temporary
folder (never the user's shared daemon, never the desktop app's private
app-server) and connects two clients, A and B, through the official protocol:

1. A starts a thread in a disposable workspace.
2. B lists the threads loaded in that server and resumes A's thread.
3. With --turn, A starts a turn that must ask for a command approval (a
   read-only sandbox that may ask), and the script records which clients
   receive the approval request and the turn's events, lets B answer it, and
   records what A then sees.

It prints only method names, client labels, ids it created and timings; no
history, titles or content of other threads. --turn uses the signed-in
account's model quota for one small turn.

    python3 -I attach_poc.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex [--turn]
"""

import base64
import json
import os
import select
import socket
import struct
import subprocess
import sys
import tempfile
import time


class Client:
    """One JSON-RPC client; the Unix transport is WebSocket, one message per
    text frame."""

    def __init__(self, label, path):
        self.label = label
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            (
                "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"
                f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
                "Sec-WebSocket-Version: 13\r\n\r\n"
            ).encode()
        )
        response = b""
        while b"\r\n\r\n" not in response:
            response += self.sock.recv(4096)
        head, self.buffer = response.split(b"\r\n\r\n", 1)
        if b" 101 " not in head.split(b"\r\n", 1)[0]:
            raise SystemExit(f"{label}: no WebSocket upgrade")
        self.partial = b""
        self.next_id = 0
        self.seen = []

    def _frame(self, opcode, payload):
        header = bytes([0x80 | opcode])
        length = len(payload)
        if length < 126:
            header += bytes([0x80 | length])
        elif length < 1 << 16:
            header += bytes([0x80 | 126]) + struct.pack("!H", length)
        else:
            header += bytes([0x80 | 127]) + struct.pack("!Q", length)
        mask = os.urandom(4)
        masked = bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
        self.sock.sendall(header + mask + masked)

    def send(self, message):
        self._frame(0x1, json.dumps(message).encode())

    def call(self, method, params):
        self.next_id += 1
        self.send({"id": self.next_id, "method": method, "params": params})
        return self.next_id

    def _frames(self):
        while True:
            if len(self.buffer) < 2:
                return
            first, second = self.buffer[0], self.buffer[1]
            length, offset = second & 0x7F, 2
            if length == 126:
                if len(self.buffer) < 4:
                    return
                length, offset = struct.unpack("!H", self.buffer[2:4])[0], 4
            elif length == 127:
                if len(self.buffer) < 10:
                    return
                length, offset = struct.unpack("!Q", self.buffer[2:10])[0], 10
            if second & 0x80:
                offset += 4
            if len(self.buffer) < offset + length:
                return
            payload = self.buffer[offset : offset + length]
            self.buffer = self.buffer[offset + length :]
            yield first & 0x80, first & 0x0F, payload

    def poll(self, timeout):
        ready, _, _ = select.select([self.sock], [], [], timeout)
        if not ready:
            return []
        chunk = self.sock.recv(1 << 20)
        if not chunk:
            raise SystemExit(f"{self.label}: server closed the connection")
        self.buffer += chunk
        messages = []
        for final, opcode, payload in self._frames():
            if opcode == 0x9:
                self._frame(0xA, payload)
            elif opcode == 0x8:
                raise SystemExit(f"{self.label}: server closed the WebSocket")
            elif opcode in (0x1, 0x0):
                self.partial += payload
                if final:
                    messages.append(json.loads(self.partial))
                    self.partial = b""
        return messages


def wait_result(clients, client, request_id, started, deadline=60):
    end = time.time() + deadline
    while time.time() < end:
        for other in clients:
            for message in other.poll(0.2):
                record(other, message, started)
                if other is client and message.get("id") == request_id and "method" not in message:
                    return message
    raise SystemExit(f"{client.label}: no answer to request {request_id}")


def record(client, message, started):
    if "method" in message:
        kind = "request" if "id" in message else "notification"
        client.seen.append((round(time.time() - started, 1), kind, message["method"], message.get("id")))


def main(program, with_turn):
    root = tempfile.mkdtemp(prefix="pastey-d2-")
    workspace = os.path.join(root, "workspace")
    os.mkdir(workspace)
    path = os.path.join(root, "app-server.sock")
    server = subprocess.Popen(
        [program, "app-server", "--listen", f"unix://{path}"]
        + (["-c", 'approval_policy="on-request"', "-c", 'sandbox_mode="read-only"'] if with_turn else []),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(100):
            if os.path.exists(path):
                break
            time.sleep(0.1)
        started = time.time()
        a, b = Client("A", path), Client("B", path)
        clients = [a, b]
        report = {}
        for client in clients:
            request = client.call(
                "initialize",
                {"clientInfo": {"name": f"pastey-d2-{client.label}", "version": "0"}, "capabilities": {}},
            )
            answer = wait_result(clients, client, request, started)
            report[f"{client.label}_initialize"] = "ok" if "result" in answer else answer.get("error")
            client.send({"method": "initialized", "params": {}})

        answer = wait_result(clients, a, a.call("thread/start", {"cwd": workspace}), started)
        thread_id = answer["result"]["thread"]["id"]
        report["A_thread_start"] = "ok"

        answer = wait_result(clients, b, b.call("thread/loaded/list", {}), started)
        loaded = answer.get("result", {}).get("data", [])
        report["B_sees_A_thread_loaded"] = thread_id in loaded or any(
            isinstance(item, dict) and item.get("id") == thread_id for item in loaded
        )
        def resume_by_b():
            answer = wait_result(
                clients, b, b.call("thread/resume", {"threadId": thread_id, "cwd": workspace}), started
            )
            result = answer.get("result", {})
            if result.get("thread", {}).get("id") != thread_id:
                return answer.get("error") or "other thread"
            turns = result.get("thread", {}).get("turns") or []
            return {"thread": "same", "turn_statuses": [turn.get("status") for turn in turns]}

        if not with_turn:
            # Before its first turn a thread has no rollout to resume.
            report["B_thread_resume_before_any_turn"] = resume_by_b()
        else:
            text = (
                "Using a single shell command, create pastey-d2.txt in this workspace "
                "containing exactly `attach`. Then reply done."
            )
            request = a.call(
                "turn/start", {"threadId": thread_id, "input": [{"type": "text", "text": text}]}
            )
            resumed = False
            approval = None
            answered_by = None
            completed = False
            end = time.time() + 240
            while time.time() < end and not completed:
                pending = []
                for client in clients:
                    for message in client.poll(0.2):
                        record(client, message, started)
                        method = message.get("method", "")
                        if method == "turn/started" and client is a and not resumed:
                            resumed = True
                            pending.append("resume")
                        if method.endswith("requestApproval") and "id" in message:
                            report.setdefault("approval_requests_to", []).append(client.label)
                            if approval is None:
                                approval = (time.time(), {})
                            approval[1][client.label] = message["id"]
                        if method == "serverRequest/resolved":
                            report.setdefault("resolved_seen_by", []).append(client.label)
                        if method == "turn/completed":
                            report.setdefault("turn_completed_seen_by", []).append(client.label)
                            report["turn_status"] = message["params"]["turn"].get("status")
                            completed = completed or client is a
                        if message.get("id") == request and client is a and "method" not in message:
                            report["A_turn_start"] = "ok" if "result" in message else message.get("error")
                if "resume" in pending:
                    # B joins the thread while A's turn runs.
                    report["B_thread_resume_during_turn"] = resume_by_b()
                if approval and answered_by is None:
                    # Let both clients receive it, then B answers if it got it;
                    # otherwise A answers after 15 s so the turn can end.
                    waited = time.time() - approval[0]
                    if "B" in approval[1] and waited > 2:
                        b.send({"id": approval[1]["B"], "result": {"decision": "accept"}})
                        answered_by = "B"
                    elif waited > 15 and "A" in approval[1]:
                        a.send({"id": approval[1]["A"], "result": {"decision": "accept"}})
                        answered_by = "A"
            report["approval_answered_by"] = answered_by
            report["file_written"] = os.path.exists(os.path.join(workspace, "pastey-d2.txt"))

        for client in clients:
            methods = {}
            for _, kind, method, _ in client.seen:
                methods[f"{kind}:{method}"] = methods.get(f"{kind}:{method}", 0) + 1
            report[f"{client.label}_received"] = methods
        print(json.dumps(report, indent=1))
    finally:
        server.terminate()
        server.wait(timeout=10)


if __name__ == "__main__":
    main(sys.argv[1], "--turn" in sys.argv[2:])
