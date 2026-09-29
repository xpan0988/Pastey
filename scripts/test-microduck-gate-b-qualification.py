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
    def probe(self, nonzero=False, wrong_identity=False, rpc_us=100, sample_us=100,
              reject_sequence=None, slow_sample=False, slow_rpc=False):
        now = [1_000_000]
        current = {}
        witnessed = set()
        calls = []
        identity = dict(controller="controller", body="body", world="world", domain="body-motion")
        self.calls = calls
        self.accepted = []
        def rpc(method, request):
            self.assertEqual(method, "robot.task")
            now[0] += rpc_us
            kind = request["kind"]
            calls.append((kind, now[0]))
            if kind == "install":
                current.clear()
                current.update(install=request["descriptor"])
            elif kind == "admit":
                current["action"] = request["descriptor"]
            elif kind == "move":
                if slow_rpc and current["install"]["epoch"] == 1 and request["descriptor"]["sequence"] == 2:
                    now[0] += 210_000
                current["move"] = copy.deepcopy(request["descriptor"])
            if kind == "status" and current.get("install", {}).get("epoch", 0) >= 3:
                self.assertIn(current["install"]["epoch"], witnessed,
                              "status must not lazily expire the probe before a loop witness")
            expired = kind == "move" and current["install"]["epoch"] >= 3 and request["descriptor"]["sequence"] == 2
            reason = "action_expired" if expired else "queued"
            if kind == "move" and current["install"]["epoch"] == 1:
                sequence = request["descriptor"]["sequence"]
                if now[0] >= current["action"]["deadline_us"]:
                    expired, reason = True, "action_expired"
                elif self.accepted and now[0] - self.accepted[-1]["native_us"] >= 200_000:
                    expired, reason = True, "refresh_lost"
                elif sequence == reject_sequence:
                    expired, reason = True, "action_expired"
                if not expired:
                    self.accepted.append(dict(sequence=sequence, native_us=now[0],
                                              action=copy.deepcopy(current["action"])))
            receipt = dict(identity={} if wrong_identity else identity, protocol="microduck-task-v1",
                           profile="reference-velocity-v1", native_us=now[0], accepted=not expired,
                           reason=reason, fenced=expired,
                           consumed_sequence=max(0, current.get("move", {}).get("sequence", 1)-1))
            self.last_receipt = receipt
            return receipt
        def sample():
            now[0] += sample_us
            if slow_sample and current.get("install", {}).get("epoch") == 1:
                now[0] += 180_000
            epoch = current.get("install", {}).get("epoch", 0)
            if epoch >= 3:
                witnessed.add(epoch)
            return dict(daemon="controller", body="body", world="world", source_us=now[0],
                        simulation_us=now[0], sequence=now[0],
                        native={"t_ns": (now[0]-1)*1000,
                                "move": {"requested": [.05, 0, 0] if nonzero else [0, 0, 0]}})
        def standing(next_sample, identities, dwell_us=200_000, minimum_native_us=0):
            result = [sample()]
            for _ in range(10):
                now[0] += 100_000
                result.append(sample())
            return result
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
        self.assertGreater(len(trace), 20)
        self.assertLessEqual(len(trace), 64)
        self.assertGreaterEqual(transcript[4]["native_us"] - transcript[0]["native_us"], 999_000)
        self.assertEqual(sum(kind == "move" for kind, _ in calls), len(self.accepted) + 8)

    def test_deadline_schedule_with_realistic_overhead_and_measured_trace(self):
        # 10ms RPC + 10ms sample: the old schedule spends >1s, even before
        # install/admit overhead. Model its first rejected refresh explicitly.
        historical = 30_000
        for sequence in range(2, 21):
            historical += 50_000 + 10_000
            if historical >= 1_000_000:
                break
            historical += 10_000
        self.assertLessEqual(sequence, 20)
        self.assertGreaterEqual(historical, 1_000_000)
        (transcript, _, _, trace), _ = self.probe(rpc_us=10_000, sample_us=10_000)
        deadline = self.accepted[0]["action"]["deadline_us"]
        self.assertLess(len(self.accepted), 20)
        self.assertEqual(len({json.dumps(r["action"], sort_keys=True) for r in self.accepted}), 1)
        self.assertTrue(all(0 < b["native_us"]-a["native_us"] < 200_000
                            for a, b in zip(self.accepted, self.accepted[1:])))
        final = self.accepted[-1]["native_us"]
        self.assertGreater(final, deadline-200_000)
        self.assertLess(final, deadline-50_000)
        self.assertEqual(transcript[3]["native_us"], final)
        self.assertGreaterEqual(transcript[4]["native_us"], deadline)
        self.assertLessEqual(deadline-transcript[0]["native_us"], 1_000_000)
        self.assertGreaterEqual(deadline-transcript[0]["native_us"], 900_000)
        self.assertGreaterEqual(trace[-1]["source_us"]-transcript[4]["native_us"], 500_000)
        self.assertTrue(any(s["source_us"] >= deadline for s in trace))
        progress = gate.SimulatorProgress()
        for s in trace:
            progress.observe(s, trace[-1]["source_us"])
        self.assertTrue(progress.completed)

    def test_too_slow_sample_fails_before_unsafe_refresh(self):
        with self.assertRaisesRegex(RuntimeError, "scheduling margin exhausted"):
            self.probe(slow_sample=True)
        self.assertEqual(len(self.accepted), 2)
        self.assertEqual(sum(kind == "status" for kind, _ in self.calls), 1)

    def test_accepted_but_over_budget_rpc_fails_closed(self):
        with self.assertRaisesRegex(RuntimeError, "accepted refresh exceeded scheduling margin"):
            self.probe(rpc_us=60_000)
        self.assertEqual(len(self.accepted), 2)
        self.assertEqual(self.calls[-1][0], "move")
        self.assertEqual(sum(kind == "status" for kind, _ in self.calls), 1)

    def test_rejection_reports_receipt_without_diagnostic_status(self):
        for options in ({"reject_sequence": 3}, {"slow_rpc": True}):
            with self.subTest(options=options):
                with self.assertRaisesRegex(RuntimeError, "refresh rejected") as caught:
                    self.probe(**options)
                data = json.loads(str(caught.exception).split(": ", 1)[1])
                previous = self.accepted[-1]
                self.assertEqual(data, dict(attempted_sequence=previous["sequence"]+1,
                    reason=self.last_receipt["reason"], native_us=self.last_receipt["native_us"],
                    action_deadline_us=previous["action"]["deadline_us"],
                    previous_accepted_sequence=previous["sequence"],
                    previous_accepted_native_us=previous["native_us"], fenced=True,
                    consumed_sequence=self.last_receipt["consumed_sequence"]))
                self.assertIn(data["reason"], ("action_expired", "refresh_lost"))
                self.assertEqual(self.calls[-1][0], "move")
                self.assertEqual(sum(kind == "status" for kind, _ in self.calls), 1)

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
