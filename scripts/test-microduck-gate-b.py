#!/usr/bin/env python3
"""Real patched robotd IPC/control-loop tests using FakeIo, never qualification."""
import copy
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest

PROTOCOL = "microduck-task-v1"
PROFILE = "reference-velocity-v1"

class NativeGateB(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="pgb-")
        self.directory = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        self.child = None
        self.connections = []
        self.addCleanup(self.cleanup_native)
        self.launch()
        self.connect()

    def cleanup_native(self):
        for stream, conn in self.connections:
            stream.close()
            conn.close()
        if self.child is not None:
            self.child.terminate()
            try:
                self.child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()

    def launch(self):
        launch = dict(environment="env", domain="body-motion", body="body", body_ref="duck", world="world")
        (self.directory / "identity.json").write_text(json.dumps(launch))
        endpoint = self.directory / "robot.sock"
        endpoint.unlink(missing_ok=True)
        self.child = subprocess.Popen([os.environ["PASTEY_GATE_B_ROBOTD"], "--fake", "--no-policy", "--socket", str(endpoint), "--pastey-task-identity", str(self.directory / "identity.json")], env={**os.environ, "DUCK_RUNTIME_ROOT": str(self.directory)}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 3
        while not endpoint.exists():
            if self.child.poll() is not None or time.monotonic() >= deadline:
                self.fail("robotd did not create its task socket")
            time.sleep(.002)

    def connect(self):
        self.conn = socket.socket(socket.AF_UNIX)
        self.conn.settimeout(2)
        self.conn.connect(str(self.directory / "robot.sock"))
        self.stream = self.conn.makefile("rwb", buffering=0)
        self.connections.append((self.stream, self.conn))
        self.counter = 0
        self.status = self.rpc("status", protocol=PROTOCOL)

    def send(self, kind, **params):
        self.counter += 1
        self.stream.write((json.dumps(dict(jsonrpc="2.0", id=self.counter, method="robot.task", params=dict(kind=kind, **params))) + "\n").encode())
        return self.counter

    def receive(self, request):
        reply = json.loads(self.stream.readline())
        self.assertEqual(reply["id"], request)
        self.assertNotIn("error", reply)
        return reply["result"]

    def rpc(self, kind, **params):
        return self.receive(self.send(kind, **params))

    def prepare(self, action_us=800_000, lease_us=2_000_000):
        status = self.rpc("status", protocol=PROTOCOL)
        self.install = dict(protocol=PROTOCOL, profile=PROFILE, identity=status["identity"], domain="body-motion", session="session", epoch=1, request="install", lease_deadline_us=status["native_us"] + lease_us)
        self.assertTrue(self.rpc("install", descriptor=self.install)["accepted"])
        self.action = dict(install=self.install, action="action", payload_digest="a" * 64, deadline_us=status["native_us"] + action_us)
        self.assertTrue(self.rpc("admit", descriptor=self.action)["accepted"])
        self.move = dict(action=self.action, request="move", sequence=1, twist=[.05, 0, 0])
        self.fence = dict(install=self.install, next_epoch=2, request="fence")

    def test_install_consumption_and_same_action_refresh(self):
        self.prepare()
        self.assertTrue(self.rpc("install", descriptor=self.install)["accepted"])
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        time.sleep(.06)
        self.assertEqual(self.rpc("status", protocol=PROTOCOL)["consumed_sequence"], 1)
        self.move["sequence"] = 2
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        self.assertEqual(self.rpc("status", protocol=PROTOCOL)["action"], self.action)

    def test_delayed_ack_fence_and_old_replay(self):
        self.prepare()
        move_ack = self.send("move", descriptor=self.move)
        fence_ack = self.send("fence", descriptor=self.fence)
        time.sleep(.06)  # native completes both while client retains old ACK unread
        self.assertTrue(self.receive(move_ack)["accepted"])
        self.assertTrue(self.receive(fence_ack)["fenced"])
        self.assertTrue(self.rpc("fence", descriptor=self.fence)["accepted"])
        self.assertFalse(self.rpc("move", descriptor=self.move)["accepted"])
        self.assertTrue(self.rpc("status", protocol=PROTOCOL)["fenced"])

    def test_expiry_without_pastey_queries_or_refresh(self):
        self.prepare(action_us=80_000)
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        time.sleep(.14)
        status = self.rpc("status", protocol=PROTOCOL)
        self.assertTrue(status["fenced"])
        self.assertEqual(status["reason"], "action_expired")
        self.assertFalse(self.rpc("move", descriptor=self.move)["accepted"])

    def test_missing_refresh_closes_before_action_deadline(self):
        self.prepare()
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        time.sleep(.27)
        self.assertEqual(self.rpc("status", protocol=PROTOCOL)["reason"], "refresh_lost")
        self.move["sequence"] = 2
        self.assertFalse(self.rpc("move", descriptor=self.move)["accepted"])

    def test_lease_expiry_without_action(self):
        status = self.rpc("status", protocol=PROTOCOL)
        self.install = dict(protocol=PROTOCOL, profile=PROFILE, identity=status["identity"], domain="body-motion", session="session", epoch=1, request="install", lease_deadline_us=status["native_us"] + 80_000)
        self.assertTrue(self.rpc("install", descriptor=self.install)["accepted"])
        time.sleep(.15)
        status = self.rpc("status", protocol=PROTOCOL)
        self.assertTrue(status["fenced"])
        self.assertEqual(status["reason"], "session_expired")
        self.assertFalse(self.rpc("install", descriptor=self.install)["accepted"])

    def test_disconnect_partial_fence_and_reconnect_do_not_resume(self):
        self.prepare()
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        self.stream.write(b'{"jsonrpc":"2.0","method":"robot.task","params":')
        self.stream.close()
        self.conn.close()
        time.sleep(.04)
        self.connect()
        self.assertTrue(self.status["fenced"])
        self.assertFalse(self.rpc("install", descriptor=self.install)["accepted"])
        self.assertFalse(self.rpc("move", descriptor=self.move)["accepted"])
        next_install = copy.deepcopy(self.install)
        next_install.update(epoch=3, session="new-session", request="new-install")
        self.assertTrue(self.rpc("install", descriptor=next_install)["accepted"])

    def test_robotd_controller_restart_invalidates_buffered_authority(self):
        self.prepare()
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        old = self.install["identity"]["controller"]
        self.child.terminate()
        self.child.wait(timeout=3)
        self.launch()
        self.connect()
        self.assertNotEqual(old, self.status["identity"]["controller"])
        self.assertIsNone(self.status["installed"])
        self.assertEqual(self.status["consumed_sequence"], 0)
        self.assertFalse(self.rpc("install", descriptor=self.install)["accepted"])
        self.assertFalse(self.rpc("move", descriptor=self.move)["accepted"])

    def test_invalid_owned_command_closes_task_window(self):
        self.prepare()
        self.assertTrue(self.rpc("move", descriptor=self.move)["accepted"])
        bad = copy.deepcopy(self.move)
        bad.update(sequence=2, twist=[.1, 0, 0])
        self.assertFalse(self.rpc("move", descriptor=bad)["accepted"])
        self.assertTrue(self.rpc("status", protocol=PROTOCOL)["fenced"])

if __name__ == "__main__":
    if not os.environ.get("PASTEY_GATE_B_ROBOTD"):
        raise SystemExit("set PASTEY_GATE_B_ROBOTD to the explicitly built pinned patched robotd")
    unittest.main()
