"""Deterministic supervisor provisioning tests; no native integration claim."""
import copy
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("gate_a", Path(__file__).with_name("microduck-gate-a.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class Provisioning(unittest.TestCase):
    def run_case(self, edit=lambda s, i: None, accepted=True, missing=False):
        calls = []
        now = [100_000]
        count = [0]
        def rpc(method, params):
            calls.append((method, params))
            return {"accepted": accepted}
        def sample():
            count[0] += 1
            now[0] += 20_000
            if missing:
                return None
            s = dict(source_us=now[0] - 1000, simulation_us=now[0], sequence=count[0],
                     daemon="daemon", body="body", world="world",
                     native={"t_ns": (now[0] - 2000) * 1000, "policy": "stand", "safety": {"fallen": False}},
                     oracle={"yaw": 0., "linear_speed": 0., "angular_speed": 0.,
                             "uncertainty": .000001, "upright": True})
            edit(s, count[0])
            return s
        try:
            gate.provision(rpc, sample, ("daemon", "body", "world"), lambda: now[0] * 1000)
        finally:
            self.assertEqual(calls, [("robot.enable", {"on": True, "toggle": False})])
        return count[0]

    def test_fresh_settled_proof_and_exactly_one_explicit_enable(self):
        self.assertEqual(self.run_case(), 11)

    def test_exact_bounded_rejection_diagnostics(self):
        with self.assertRaises(gate.AcquisitionError) as rejected:
            self.run_case(lambda s, i: s["native"].update(t_ns=120_000_000))
        data = rejected.exception.data
        self.assertEqual(data, dict(enabled_us=100000, source_us=119000, now_us=120000,
                                   native_us=120000, simulation_us=120000, sequence=1,
                                   source_minus_enabled_us=19000, native_minus_source_us=1000,
                                   now_minus_source_us=1000,
                                   failed_conditions=["0 <= source_us - native_us < 20000"]))
        self.assertEqual(json.loads(str(rejected.exception).split(": ", 1)[1]), data)
        self.assertLess(len(str(rejected.exception)), 600)
        for secret in ("daemon", "body", "world", "permit", "policy"):
            self.assertNotIn(secret, data)

    def test_skew_boundary_and_stale_acquisition_reject(self):
        for edit, condition in (
            (lambda s, i: s["native"].update(t_ns=(s["source_us"] - 20000) * 1000),
             "0 <= source_us - native_us < 20000"),
            (lambda s, i: s.update(source_us=s["source_us"] - 200000),
             "0 <= now_us - source_us < 200000"),
        ):
            with self.subTest(condition=condition), self.assertRaises(gate.AcquisitionError) as rejected:
                self.run_case(edit)
            self.assertIn(condition, rejected.exception.data["failed_conditions"])

    def test_queued_pre_enable_frames_are_not_proof(self):
        def queued(s, i):
            if i <= 2:
                s.update(source_us=99000)
                s["native"]["t_ns"] = 98000000
        with contextlib.redirect_stderr(io.StringIO()) as diagnostics:
            self.assertEqual(self.run_case(queued), 13)  # Full dwell starts after tail.
        self.assertEqual(len(diagnostics.getvalue().splitlines()), 1)
        self.assertIn('"source_minus_enabled_us":-1000', diagnostics.getvalue())
        # A frame that started before ACK but acquired afterward is still excluded.
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.run_case(lambda s, i: s["native"].update(t_ns=99999000) if i == 1 else None), 12)
            with self.assertRaises(gate.AcquisitionError):
                self.run_case(lambda s, i: (s.update(source_us=99000), s["native"].update(t_ns=98000000)))

    def test_cold_limp_and_ack_alone_cannot_qualify(self):
        with self.assertRaisesRegex(RuntimeError, "timeout"):
            self.run_case(lambda s, i: s["native"].update(policy="limp"))

    def test_enable_refusal(self):
        with self.assertRaisesRegex(RuntimeError, "refused"):
            self.run_case(accepted=False)

    def test_observation_loss(self):
        with self.assertRaisesRegex(RuntimeError, "lost"):
            self.run_case(missing=True)

    def test_incarnation_replacement(self):
        for field in ("daemon", "body", "world"):
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, "replaced"):
                self.run_case(lambda s, i: s.update({field: "replacement"}))

    def test_stale_or_cached_source(self):
        for field in ("source_us", "sequence"):
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.run_case(lambda s, i: s.update({field: 119000 if field == "source_us" else 1}))

    def test_paused_simulator(self):
        with self.assertRaisesRegex(RuntimeError, "paused"):
            self.run_case(lambda s, i: s.update(simulation_us=120000))

    def test_native_clock_reset_and_source_gap(self):
        with self.assertRaisesRegex(RuntimeError, "acquisition"):
            self.run_case(lambda s, i: s["native"].update(t_ns=1))
        with self.assertRaises(RuntimeError):
            self.run_case(lambda s, i: s.update(source_us=s["source_us"] - (200000 if i == 2 else 0)))

    def test_non_advancing_native_clock_rejects_even_inside_skew_bound(self):
        def cached(s, i):
            if i == 2:
                s["source_us"] -= 2000
                s["native"]["t_ns"] = 118000000
        with self.assertRaises(gate.AcquisitionError) as rejected:
            self.run_case(cached)
        self.assertEqual(rejected.exception.data["failed_conditions"],
                         ["native_t_ns > previous_native_t_ns"])

    def test_unsettled_uncertain_or_wrong_controller_times_out(self):
        for field, value in (("upright", False), ("uncertainty", .01), ("linear_speed", .03),
                             ("angular_speed", .2), ("yaw", .1)):
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, "timeout"):
                self.run_case(lambda s, i: s["oracle"].update({field: value}))

    def test_task_adapter_has_no_enable_and_preparation_precedes_seal(self):
        source = Path(gate.__file__).read_text()
        self.assertLess(source.index("provision(rpc, next_sample"), source.index("emit(dict(version=1"))
        rust = Path("src-tauri/src/physical/adapters/microduck.rs").read_text()
        self.assertNotIn("robot.enable", rust)
        request = rust.split("enum RequestV1 {")[1].split("}")[0]
        self.assertNotIn("Enable", request)
        # Preparation receives no Root/session/action/Core argument or handle.
        self.assertEqual(gate.provision.__code__.co_varnames[:4], ("rpc", "next_sample", "identities", "clock"))


class AcquisitionCorrelation(unittest.TestCase):
    def record(self, us, seq=1, oracle=None):
        return dict(acquisition_ns=us*1000, source_us=us, simulation_us=us,
                    sequence=seq, oracle=oracle)

    def test_real_read_start_then_body_acquisition_and_quantization(self):
        self.assertTrue(gate.same_control_frame(100000000, 101000))
        self.assertTrue(gate.same_control_frame(100000999, 100000))
        self.assertFalse(gate.same_control_frame(100001000, 100000))
        self.assertTrue(gate.same_control_frame(100000000, 119999))
        self.assertFalse(gate.same_control_frame(100000000, 120000))
        sample = gate.correlate_acquisition([self.record(99000), self.record(101000, 2)], 100000000, 102000)
        self.assertEqual(sample["sequence"], 2)
        self.assertNotIn("acquisition_ns", sample)
        rounded = self.record(100000)
        rounded["acquisition_ns"] += 999
        self.assertEqual(gate.correlate_acquisition([rounded], 100000001, 100001)["source_us"], 100000)

    def test_first_read_including_missing_oracle_cannot_be_skipped_for_later_frame(self):
        records = [self.record(101000), self.record(110000, 2, {"upright": True})]
        sample = gate.correlate_acquisition(records, 100000000, 111000)
        self.assertEqual(sample["sequence"], 1)
        self.assertIsNone(sample["oracle"])
        # Repeated/coasted native timestamp returns the same acquisition, never newest.
        self.assertEqual(gate.correlate_acquisition(records, 100000000, 112000)["sequence"], 1)
        with self.assertRaises(gate.AcquisitionError):
            gate.correlate_acquisition([self.record(121000)], 100000000, 122000)

    def test_stale_missing_or_prior_read_cannot_correlate(self):
        for records, now in (([self.record(101000)], 301000),
                             ([self.record(99000)], 102000), ([], 102000)):
            with self.subTest(records=records), self.assertRaises(gate.AcquisitionError):
                gate.correlate_acquisition(records, 100000000, now)

    def test_evicted_read_cannot_be_replaced_by_later_unrelated_read(self):
        # Even a later read within 20 ms and 200 ms cannot replace an evicted match.
        records = [self.record(110000, 33)]
        with self.assertRaises(gate.AcquisitionError) as rejected:
            gate.correlate_acquisition(records, 100000000, 111000, evicted_ns=101000000)
        self.assertEqual(rejected.exception.data["failed_conditions"],
                         ["native_read_start > evicted_acquisition_ns"])
        self.assertEqual(gate.correlate_acquisition(records, 109000000, 111000,
                                                   evicted_ns=101000000)["sequence"], 33)

    def test_mapping_error_preserves_enable_and_signed_diagnostics(self):
        now = [100000]
        def sample():
            now[0] = 140000
            return gate.correlate_acquisition([self.record(130000)], 110000000, now[0])
        with self.assertRaises(gate.AcquisitionError) as rejected:
            gate.provision(lambda *args: {"accepted": True}, sample, ("d", "b", "w"), lambda: now[0]*1000)
        self.assertEqual(rejected.exception.data["enabled_us"], 100000)
        self.assertEqual(rejected.exception.data["native_minus_source_us"], -20000)

    def test_standing_dwell_excludes_frame_started_before_causal_cutoff(self):
        frames = iter((100500, 120000, 220000, 320000))
        def sample():
            source = next(frames)
            return dict(source_us=source, daemon="d", body="b", world="w",
                        native={"t_ns": (source-1000)*1000},
                        oracle=dict(upright=True, yaw=0., linear_speed=0.,
                                    angular_speed=0., uncertainty=.000001))
        with patch.object(gate.time, "monotonic_ns", return_value=400000000):
            result = gate.collect_standing(sample, ("d", "b", "w"), minimum_native_us=100000)
        self.assertEqual([s["source_us"] for s in result], [120000, 220000, 320000])


if __name__ == "__main__":
    unittest.main()
