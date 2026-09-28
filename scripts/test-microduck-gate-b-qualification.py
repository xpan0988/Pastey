"""Deterministic producer orchestration checks, never simulator qualification."""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("qualification", Path(__file__).with_name("microduck-gate-a.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class QualificationProducer(unittest.TestCase):
    def probe(self, nonzero=False, wrong_identity=False):
        now = [1_000_000]
        current = {}
        witnessed = set()
        calls = []
        identity = dict(controller="controller", body="body", world="world", domain="body-motion")
        def rpc(method, request):
            self.assertEqual(method, "robot.task")
            now[0] += 100
            kind = request["kind"]
            calls.append((kind, now[0]))
            if kind == "install":
                current.clear()
                current.update(install=request["descriptor"])
            elif kind == "admit":
                current["action"] = request["descriptor"]
            elif kind == "move":
                current["move"] = copy.deepcopy(request["descriptor"])
            if kind == "status" and current.get("install", {}).get("epoch", 0) >= 3:
                self.assertIn(current["install"]["epoch"], witnessed,
                              "status must not lazily expire the probe before a loop witness")
            expired = kind == "move" and current["install"]["epoch"] >= 3 and request["descriptor"]["sequence"] == 2
            return dict(identity={} if wrong_identity else identity, protocol="microduck-task-v1",
                        profile="reference-velocity-v1", native_us=now[0], accepted=not expired)
        def sample():
            now[0] += 100
            epoch = current.get("install", {}).get("epoch", 0)
            if epoch >= 3:
                witnessed.add(epoch)
            return dict(daemon="controller", body="body", world="world", source_us=now[0],
                        native={"t_ns": (now[0]-1)*1000,
                                "move": {"requested": [.05, 0, 0] if nonzero else [0, 0, 0]}})
        def standing(next_sample, identities, dwell_us=200_000, minimum_native_us=0):
            now[0] += dwell_us
            return [sample()]
        def sleep(seconds):
            now[0] += int(seconds * 1_000_000)
        with patch.object(gate.time, "monotonic_ns", lambda: now[0] * 1000), \
                patch.object(gate, "collect_standing", standing), patch.object(gate.os, "kill") as kill:
            result = gate.native_probes(rpc, sample, type("Process", (), {"pid": 123})(), identity, sleep)
            self.assertEqual(kill.call_count, 2)
        return result, calls

    def test_reference_refresh_and_pre_status_expiry_witnesses(self):
        (transcript, witnesses, setups, trace), calls = self.probe()
        self.assertEqual(len(transcript), 13)
        self.assertEqual(len(witnesses), 4)
        self.assertEqual(len(setups), 4)
        self.assertEqual(len(trace), 21)
        self.assertGreaterEqual(transcript[4]["native_us"] - transcript[0]["native_us"], 999_000)
        self.assertEqual(sum(kind == "move" for kind, _ in calls), 28)

    def test_loop_input_not_closed_cannot_be_repaired_by_status(self):
        with self.assertRaisesRegex(RuntimeError, "independently discard"):
            self.probe(nonzero=True)

    def test_wrong_native_identity_denies_before_installation(self):
        with self.assertRaisesRegex(RuntimeError, "identity/protocol"):
            self.probe(wrong_identity=True)

    def test_artifact_change_and_unpinned_native_runtime_deny(self):
        with tempfile.TemporaryDirectory() as directory:
            params, walk, stand, ort = [Path(directory) / name for name in ("params", "walk", "stand", "ort")]
            for p in (params, walk, stand, ort):
                p.write_bytes(p.name.encode())
            sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
            pins = dict(paramsSha256=sha(params), policySha256=[sha(walk), sha(stand)], onnxRuntimeSha256=sha(ort))
            with patch.dict(os.environ, ORT_DYLIB_PATH=str(ort)):
                gate.check_native_artifacts(pins, params, [walk, stand])
                walk.write_bytes(b"replacement")
                with self.assertRaisesRegex(RuntimeError, "artifact changed"):
                    gate.check_native_artifacts(pins, params, [walk, stand])
                walk.write_bytes(b"walk")
                ort.write_bytes(b"replacement")
                with self.assertRaisesRegex(RuntimeError, "ONNX Runtime mismatch"):
                    gate.check_native_artifacts(pins, params, [walk, stand])

    def test_profile_cannot_enable_posture_or_take_unlisted_policy(self):
        with tempfile.TemporaryDirectory() as directory:
            params = Path(directory) / "robotd.toml"
            params.write_text('[policy]\nenabled=true\nwalk="wrong"\nstand="wrong"\n')
            with self.assertRaisesRegex(RuntimeError, "locators"):
                gate.snapshot_native_artifacts(params, json.dumps(["walk", "stand"]))
            params.write_text('[policy]\nenabled=true\nwalk="walk"\nstand="stand"\n')
            with self.assertRaisesRegex(RuntimeError, "disabled"):
                gate.snapshot_native_artifacts(params, json.dumps(["walk", "stand"]))


if __name__ == "__main__":
    unittest.main()
