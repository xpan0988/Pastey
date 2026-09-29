"""Deterministic producer orchestration checks, never simulator qualification."""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("qualification", Path(__file__).with_name("microduck-gate-a.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

# Fixture reads the single Rust protocol constant; the production owned Rust
# launcher serializes that same REFERENCE_TWIST into the supervisor arguments.
protocol = Path(__file__).resolve().parents[1] / "native/microduck/overlay/duck-ipc-proto/src/task_authority.rs"
REFERENCE_FORWARD_MPS = float(re.search(
    r"^pub const REFERENCE_FORWARD_MPS: f64 = ([0-9.]+);$", protocol.read_text(), re.M).group(1))
REFERENCE_TWIST = [REFERENCE_FORWARD_MPS, 0.0, 0.0]


class QualificationProducer(unittest.TestCase):
    def test_reference_has_deliberate_margin_and_early_ema_crossing(self):
        threshold, alpha, hz = 0.05, 0.2, 50
        self.assertGreaterEqual(REFERENCE_FORWARD_MPS, threshold * 1.5)
        self.assertLess(REFERENCE_FORWARD_MPS, 0.1)
        old, reference, first_walk_tick = 0.0, 0.0, None
        for tick in range(1, hz + 1):
            old += alpha * (0.05 - old)
            reference += alpha * (REFERENCE_FORWARD_MPS - reference)
            self.assertLessEqual(old, threshold)
            if reference > threshold and first_walk_tick is None:
                first_walk_tick = tick
        self.assertEqual(first_walk_tick, 5)
        self.assertGreaterEqual(1_000_000 - first_walk_tick * 1_000_000 // hz, 900_000)

    def test_qualification_requires_owned_protocol_payload_without_a_default(self):
        argv = ["supervisor", "robotd", "rl", "params", "[]", "controller", "body", "world",
                json.dumps({"version": 1}), "environment", "body-motion", "body-ref"]
        with patch.object(gate.sys, "argv", argv):
            with self.assertRaisesRegex(RuntimeError, "owned native reference payload required"):
                gate.run()

    def probe(self, nonzero=False, wrong_identity=False, rpc_us=100, sample_us=100,
              reject_sequence=None, slow_sample=False, slow_rpc=False, deceleration_us=0):
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
                self.assertEqual(request["descriptor"]["twist"], REFERENCE_TWIST)
                if slow_rpc and current["install"]["epoch"] == 1 and request["descriptor"]["sequence"] == 2:
                    now[0] += 210_000
                current["move"] = copy.deepcopy(request["descriptor"])
            elif kind == "fence":
                current["fence_us"] = now[0]
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
            moving = (epoch == 1 and "fence_us" in current
                      and now[0] - current["fence_us"] <= deceleration_us)
            return dict(daemon="controller", body="body", world="world", source_us=now[0],
                        simulation_us=now[0], sequence=now[0],
                        native={"t_ns": (now[0]-1)*1000,
                                "move": {"requested": REFERENCE_TWIST if nonzero else [0, 0, 0]},
                                "policy": "stand", "safety": {"fallen": False}},
                        oracle=dict(position=[.04, 0., .125], yaw=0., upright=True,
                                    linear_speed=.04 if moving else 0., angular_speed=.2 if moving else 0.,
                                    uncertainty=1e-6))
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
            result = gate.native_probes(rpc, sample, type("Process", (), {"pid": 123})(),
                                       identity, REFERENCE_TWIST, sleep)
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

    def test_reference_action_keeps_more_than_200_ms_of_post_fence_deceleration(self):
        (transcript, _, _, trace), _ = self.probe(rpc_us=1_000, sample_us=20_000,
                                                deceleration_us=320_000)
        fence = transcript[4]["native_us"]
        transition = [s for s in trace if s["source_us"] >= fence and s["oracle"]["linear_speed"] > .02]
        self.assertGreater(transition[-1]["source_us"] - fence, 200_000)
        self.assertTrue(all(s["oracle"]["angular_speed"] > .1 for s in transition))
        self.assertLessEqual(len(trace), 64)
        self.assertTrue(all(0 < b["source_us"]-a["source_us"] < 200_000
                            and b["native"]["t_ns"] > a["native"]["t_ns"] and b["sequence"] > a["sequence"]
                            for a, b in zip(trace, trace[1:])))
        progress = gate.SimulatorProgress()
        for s in trace:
            progress.observe(s, trace[-1]["source_us"])
        self.assertTrue(progress.completed)

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


class ReferenceSettling(unittest.TestCase):
    @staticmethod
    def frame(source, sequence, linear=0., angular=0.):
        return dict(daemon="d", body="b", world="w", source_us=source, simulation_us=source,
                    sequence=sequence, native=dict(t_ns=(source-1)*1000, policy="walk", safety={"fallen": False}),
                    oracle=dict(position=[.04, 0., .125], yaw=.386, upright=True,
                                linear_speed=linear, angular_speed=angular, uncertainty=1e-6))

    def collect(self, events, prefix_count=11, mutate=None, standing=False, cutoff_offset=1):
        self.prefix = [self.frame(1_000_000+i*100_000, i+1, .08 if i else 0.) for i in range(prefix_count)]
        source, sequence = [self.prefix[-1]["source_us"]], [self.prefix[-1]["sequence"]]
        now, self.acquired = [source[0]], []
        events = iter(events)
        def sample():
            event = next(events)
            if isinstance(event, Exception):
                raise event
            delta, linear, angular = event
            source[0] += delta
            sequence[0] += 1
            s = self.frame(source[0], sequence[0], linear, angular)
            if mutate:
                mutate(s, len(self.acquired))
            now[0] = source[0]
            self.acquired.append(s)
            return s
        with patch.object(gate.time, "monotonic_ns", lambda: now[0]*1000):
            if standing:
                return gate.collect_standing(sample, ("d", "b", "w"), 500_000, source[0]+cutoff_offset)
            return gate.collect_reference_settling(sample, ("d", "b", "w"), self.prefix, source[0]+cutoff_offset)

    def assert_continuous(self, trace):
        self.assertLessEqual(len(trace), 64)
        for a, b in zip(trace, trace[1:]):
            self.assertTrue(0 < b["source_us"]-a["source_us"] < 200_000)
            self.assertGreater(b["native"]["t_ns"], a["native"]["t_ns"])
            self.assertGreater(b["sequence"], a["sequence"])
        progress = gate.SimulatorProgress()
        for s in trace:
            progress.observe(s, trace[-1]["source_us"])
        self.assertTrue(progress.completed)
        for s in trace[len(self.prefix):]:
            self.assertTrue(any(s is acquired for acquired in self.acquired), "only real frames may be retained")

    def test_transitional_frames_bridge_deceleration_without_a_splice(self):
        trace = self.collect([(20_000, .08, .2)]*16 + [(20_000, .02, .1)]*30)
        self.assert_continuous(trace)
        self.assertEqual(trace[:len(self.prefix)], self.prefix)
        self.assertIs(trace[len(self.prefix)], self.acquired[0])
        moving = [s for s in trace[len(self.prefix):] if not all(gate.standing_checks(s).values())]
        self.assertGreater(moving[-1]["source_us"]-self.prefix[-1]["source_us"], 200_000)
        resting = next(s for s in trace[len(self.prefix):] if all(gate.standing_checks(s).values()))
        self.assertEqual(trace[-1]["source_us"]-resting["source_us"], 500_000)

    def test_sub_cadence_speed_interruptions_reset_the_real_settling_interval(self):
        for linear, angular in ((.020001, .1), (.02, .100001)):
            with self.subTest(linear=linear, angular=angular):
                trace = self.collect([(20_000, .08, .2)]*15 + [(20_000, .02, .1)]*24
                                     + [(20_000, linear, angular)] + [(20_000, .02, .1)]*30)
                self.assert_continuous(trace)
                self.assertTrue(any(s is self.acquired[39] for s in trace))
                self.assertTrue(any(s is self.acquired[40] for s in trace))
                self.assertEqual(trace[-1]["source_us"]-self.acquired[40]["source_us"], 500_000)
                self.assertEqual(len(self.acquired), 66)

    def test_499999_us_of_rest_is_insufficient_and_500000_us_is_required(self):
        trace = self.collect([(20_000, .02, .1)]*25 + [(19_999, .02, .1), (1, .02, .1)])
        self.assertEqual(len(self.acquired), 27)
        self.assertEqual(trace[-1]["source_us"]-self.acquired[0]["source_us"], 500_000)
        self.assertIs(trace[-1], self.acquired[-1])
        self.assert_continuous(trace)

    def test_native_cutoff_and_complete_progress_window_are_still_required(self):
        trace = self.collect([(20_000, 0., 0.)]*60, prefix_count=1, cutoff_offset=20_000)
        self.assertLess(self.acquired[0]["native"]["t_ns"]//1000, self.prefix[-1]["source_us"]+20_000)
        self.assertIs(trace[1], self.acquired[1])
        self.assertGreaterEqual(trace[-1]["source_us"]-trace[0]["source_us"], 1_000_000)
        self.assert_continuous(trace)

    def test_thinning_bridges_only_with_real_acquisitions(self):
        # The second 180 ms acquisition would make a 270 ms retained gap.
        trace = self.collect([(90_000, .08, .2), (90_000, .08, .2), (180_000, .08, .2)]
                             + [(90_000, 0., 0.)]*10)
        self.assertTrue(any(s is self.acquired[1] for s in trace))
        self.assert_continuous(trace)

    def test_genuine_200_ms_acquisition_gap_fails_closed(self):
        for gap in (200_000, 200_001):
            with self.subTest(gap=gap), self.assertRaises(gate.AcquisitionError) as caught:
                self.collect([(20_000, .08, .2)]*3 + [(gap, 0., 0.)])
            self.assertEqual(caught.exception.data["source_delta_us"], gap)
            self.assertIn("0 < source_delta_us < 200000", caught.exception.data["failed_conditions"])

    def test_unretained_raw_frames_cannot_hide_regression_or_replacement(self):
        def cached_source(s):
            s["source_us"] = self.acquired[-1]["source_us"]
            s["native"]["t_ns"] = self.acquired[-1]["native"]["t_ns"]
        edits = [(cached_source, "source_delta_us"),
                 (lambda s: s.update(sequence=self.acquired[-1]["sequence"]), "sequence >"),
                 (lambda s: s["native"].update(t_ns=self.acquired[-1]["native"]["t_ns"]), "native_t_ns >"),
                 (lambda s: s.update(simulation_us=self.acquired[-1]["simulation_us"]-1), "simulation_us >="),
                 (lambda s: s.update(world="replacement"), "incarnation changed"),
                 (lambda s: s["oracle"].update(upright=False), "oracle_upright"),
                 (lambda s: s["native"]["safety"].update(fallen=True), "native_not_fallen")]
        for edit, message in edits:
            with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                self.collect([(10_000, .08, .2)]*3, mutate=lambda s, i: edit(s) if i == 1 else None)
        with self.assertRaisesRegex(RuntimeError, "stale native acquisition"):
            self.collect([(20_000, .08, .2), RuntimeError("stale native acquisition")])

    def test_evidence_limit_fails_without_trimming_or_splicing(self):
        with self.assertRaisesRegex(RuntimeError, "evidence limit exceeded \\(64 samples\\)"):
            self.collect([(20_000, .08, .2)]*400)
        self.assertEqual(len(self.prefix), 11)
        self.assertLess(self.acquired[-1]["source_us"]-self.prefix[-1]["source_us"], 10_000_000)

    def test_pure_standing_collection_still_discards_the_moving_prefix(self):
        trace = self.collect([(20_000, .08, .2)]*16 + [(20_000, .02, .1)]*55, standing=True)
        self.assertTrue(all(all(gate.standing_checks(s).values()) for s in trace))
        self.assertIs(trace[0], self.acquired[16])
        self.assertGreaterEqual(trace[-1]["source_us"]-trace[0]["source_us"], 1_000_000)
        self.assertLessEqual(len(trace), 32)


if __name__ == "__main__":
    unittest.main()
