"""Deterministic supervisor provisioning tests; no native integration claim."""
import ast
import copy
import contextlib
import importlib.util
import io
import json
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("gate_a", Path(__file__).with_name("microduck-gate-a.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
PROGRESS_GOLDEN = json.loads((Path(__file__).parent / "fixtures/microduck-simulator-progress-v1.json").read_text())


def progress_records(case):
    source, sim, seq = PROGRESS_GOLDEN["initialSourceUs"], case.get("initialSimulationUs", PROGRESS_GOLDEN["initialSimulationUs"]), 1
    native = (source-case.get("initialNativeSourceSkewUs", PROGRESS_GOLDEN["nativeSourceSkewUs"]))*1000
    def record():
        return dict(source_us=source, simulation_us=sim, sequence=seq,
                    daemon="daemon", body="body", world="world",
                    native=dict(t_ns=native,
                                policy="stand", safety={"fallen": False}),
                    oracle=dict(upright=True, yaw=0., linear_speed=0., angular_speed=0., uncertainty=.000001))
    yield record()
    for step in case["steps"]:
        ds, dt, repeat = step[:3]
        for _ in range(repeat):
            source, sim, seq = source+ds, sim+dt, seq+1
            native += (step[3] if len(step) > 3 else ds)*1000
            yield record()


class Provisioning(unittest.TestCase):
    def run_case(self, edit=lambda s, i: None, accepted=True, missing=False, components=None):
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
            gate.provision(rpc, sample, ("daemon", "body", "world"), lambda: now[0] * 1000,
                           settling_components=components)
        finally:
            self.assertEqual(calls, [("robot.enable", {"on": True, "toggle": False})])
        return count[0]

    def test_fresh_settled_proof_and_exactly_one_explicit_enable(self):
        self.assertEqual(self.run_case(), 51)

    def test_standing_headings_and_predicate_parity(self):
        # Run 007 values, plus both signs and headings beyond the atan2 range.
        for yaw in (.3861663504503868, -2.4, 3.1, -8., 8.):
            def edit(s, i):
                s["oracle"].update(yaw=yaw, linear_speed=.0004905021411921403,
                                   angular_speed=.003995644020477725)
            with self.subTest(yaw=yaw):
                self.assertEqual(self.run_case(edit), 51)
                self.collect_case(edit)
        for yaw in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(yaw=yaw):
                edit = lambda s, i: s["oracle"].update(yaw=yaw)
                with self.assertRaises(gate.SettlingTimeout):
                    self.run_case(edit)
                with self.assertRaisesRegex(RuntimeError, "standing qualification unavailable"):
                    self.collect_case(edit)

    def collect_case(self, edit):
        now = [100000]
        count = [0]
        def sample():
            count[0] += 1
            now[0] += 20000
            s = dict(source_us=now[0]-1000, simulation_us=now[0], sequence=count[0],
                     daemon="daemon", body="body", world="world",
                     native=dict(t_ns=(now[0]-2000)*1000, policy="stand", safety={"fallen": False}),
                     oracle=dict(yaw=.386, upright=True, linear_speed=0., angular_speed=0., uncertainty=1e-6))
            edit(s, count[0])
            return s
        with patch.object(gate.time, "monotonic_ns", side_effect=lambda: now[0]*1000):
            return gate.collect_standing(sample, ("daemon", "body", "world"))

    def test_collection_keeps_all_standing_guards(self):
        edits = [lambda s, i: s["native"].update(policy="homing"),
                 lambda s, i: s["native"]["safety"].update(fallen=True),
                 lambda s, i: s["oracle"].update(upright=False)]
        for key in ("linear_speed", "angular_speed", "uncertainty"):
            for value in (-.01, float("nan"), float("inf")):
                edits.append(lambda s, i, k=key, v=value: s["oracle"].update({k: v}))
        edits += [lambda s, i: s["oracle"].update(angular_speed=.100001),
                  lambda s, i: s["oracle"].update(linear_speed=.020001),
                  lambda s, i: s["oracle"].update(uncertainty=.001001)]
        for edit in edits:
            with self.subTest(edit=edit):
                with self.assertRaises(gate.SettlingTimeout):
                    self.run_case(edit)
                with self.assertRaisesRegex(RuntimeError, "standing qualification unavailable"):
                    self.collect_case(edit)
        # HOME_RAMP/policy transition is permitted before continuous settled proof.
        records = self.collect_case(lambda s, i: s["native"].update(policy="homing" if i < 100 else "walk"))
        self.assertGreaterEqual(records[0]["sequence"], 100)

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
            self.assertEqual(self.run_case(queued), 53)  # Full progress window starts after tail.
        self.assertEqual(len(diagnostics.getvalue().splitlines()), 1)
        self.assertIn('"source_minus_enabled_us":-1000', diagnostics.getvalue())
        # A frame that started before ACK but acquired afterward is still excluded.
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.run_case(lambda s, i: s["native"].update(t_ns=99999000) if i == 1 else None), 52)
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
                             ("angular_speed", .2), ("yaw", float("nan"))):
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, "timeout"):
                self.run_case(lambda s, i: s["oracle"].update({field: value}))

    def timeout_summary(self, edit, components=None):
        with self.assertRaises(gate.SettlingTimeout) as rejected:
            self.run_case(edit, components=components)
        error = rejected.exception
        self.assertEqual(json.loads(str(error).split(": ", 1)[1]), error.data)
        self.assertLess(len(str(error)), 2500)
        self.assertEqual(error.data["valid_acquisition_count"], 500)
        self.assertTrue(error.data["progress_window_completed"])
        self.assertEqual(error.data["progress_window_completed_source_us"], 1119000)
        self.assertEqual(error.data["progress_window_completed_after_enable_us"], 1019000)
        return error.data

    def test_timeout_distinguishes_each_settling_failure(self):
        cases = [
            (lambda s, i: s.update(oracle=None),
             {"oracle_available_finite", "oracle_upright", "yaw_finite",
              "linear_speed_in_0_to_0_02", "angular_speed_in_0_to_0_1", "uncertainty_in_0_to_0_001"}),
            (lambda s, i: s["oracle"].update(upright=False), {"oracle_upright"}),
            (lambda s, i: s["oracle"].update(yaw=float("nan")), {"oracle_available_finite", "yaw_finite"}),
            (lambda s, i: s["oracle"].update(linear_speed=.03), {"linear_speed_in_0_to_0_02"}),
            (lambda s, i: s["oracle"].update(angular_speed=.2), {"angular_speed_in_0_to_0_1"}),
            (lambda s, i: s["oracle"].update(uncertainty=.01), {"uncertainty_in_0_to_0_001"}),
            (lambda s, i: s["native"].update(policy="limp"), {"native_policy_stand_walk"}),
            (lambda s, i: s["native"]["safety"].update(fallen=True), {"native_not_fallen"}),
        ]
        for edit, failures in cases:
            with self.subTest(failures=failures):
                data = self.timeout_summary(edit)
                self.assertEqual(data["failure_counts"],
                                 {k: 500 if k in failures else 0 for k in data["failure_counts"]})
                self.assertFalse(data["settling_predicate_ever_true"])
                self.assertEqual(data["longest_settling_streak_us"], 0)
        # Native not-fallen never substitutes for Pastey's stricter oracle upright.
        self.assertEqual(self.timeout_summary(cases[1][0])["failure_counts"]["native_not_fallen"], 0)

    def test_timeout_tracks_interrupted_streak_and_finite_extrema(self):
        data = self.timeout_summary(lambda s, i: s["oracle"].update(yaw=-.1 if i % 10 == 0 else 0.,
                                                                       angular_speed=.2 if i % 10 == 0 else 0.))
        self.assertTrue(data["settling_predicate_ever_true"])
        self.assertEqual(data["longest_settling_streak_us"], 160000)  # 9 reads, below unchanged dwell.
        self.assertEqual(data["failure_counts"]["angular_speed_in_0_to_0_1"], 50)
        self.assertEqual(data["metrics"]["yaw"], dict(final=-.1, min=-.1, max=0., clamped=False))
        self.assertEqual(data["metrics"]["abs_yaw"], dict(final=.1, min=0., max=.1, clamped=False))

    def test_timeout_reports_metric_extrema_and_preserves_threshold_boundaries(self):
        def edit(s, i):
            s["native"]["safety"]["fallen"] = True  # Keep this a timeout, independently of metrics.
            s["oracle"].update(yaw=(-1e-6, 0., 1e-6)[i % 3],
                              linear_speed=(.01, .03, .02)[i % 3],
                              angular_speed=(.05, .2, .1)[i % 3],
                              uncertainty=(.0001, .002, .001)[i % 3])
        data = self.timeout_summary(edit)
        for field, final, low, high in (("yaw", 1e-6, -1e-6, 1e-6), ("abs_yaw", 1e-6, 0., 1e-6),
                                       ("linear_speed", .02, .01, .03), ("angular_speed", .1, .05, .2),
                                       ("uncertainty", .001, .0001, .002)):
            self.assertEqual(data["metrics"][field], dict(final=final, min=low, max=high, clamped=False))
        self.assertEqual(data["failure_counts"]["yaw_finite"], 0)
        for key in ("linear_speed_in_0_to_0_02", "angular_speed_in_0_to_0_1", "uncertainty_in_0_to_0_001"):
            self.assertEqual(data["failure_counts"][key], 167)

    def test_timeout_nonfinite_missing_and_clamped_values_are_bounded(self):
        for field in ("yaw", "linear_speed", "angular_speed", "uncertainty"):
            for value in (float("nan"), float("inf"), None):
                with self.subTest(field=field, value=value):
                    data = self.timeout_summary(lambda s, i: s["oracle"].update({field: value}))
                    self.assertEqual(data["failure_counts"]["oracle_available_finite"], 500)
                    self.assertEqual(data["metrics"][field], dict(final=None, min=None, max=None, clamped=False))
        data = self.timeout_summary(lambda s, i: s["oracle"].update(yaw=-1e100, linear_speed=1e100,
                                                                    angular_speed=1e100, uncertainty=1e100))
        for field in ("yaw", "abs_yaw", "linear_speed", "angular_speed", "uncertainty"):
            self.assertTrue(data["metrics"][field]["clamped"])
            self.assertLessEqual(abs(data["metrics"][field]["final"]), 1e6)

    def test_timeout_upright_components_remain_local_and_report_final_best(self):
        data = self.timeout_summary(lambda s, i: s["oracle"].update(upright=False),
                                    components=lambda s: dict(gravity_z=-.6-.1*(s["sequence"] % 5),
                                                              trunk_height=.04+.01*(s["sequence"] % 5)))
        self.assertAlmostEqual(data["metrics"]["gravity_z"]["final"], -.6)
        self.assertAlmostEqual(data["metrics"]["gravity_z"]["min"], -1.)
        self.assertAlmostEqual(data["metrics"]["trunk_height"]["final"], .04)
        self.assertAlmostEqual(data["metrics"]["trunk_height"]["max"], .08)
        record = dict(acquisition_ns=101000000, source_us=101000, simulation_us=100000, sequence=1,
                      oracle={"upright": False}, _settling_components={"gravity_z": -.6, "trunk_height": .04})
        acquired = gate.correlate_acquisition([record], 100000000, 102000)
        self.assertEqual(set(acquired), {"source_us", "simulation_us", "sequence", "oracle", "native"})
        self.assertEqual(acquired["oracle"], {"upright": False})

    def test_timeout_excludes_arbitrary_native_and_oracle_content(self):
        secret = "secret-content/permit/path/identity" * 10000
        def edit(s, i):
            s["native"].update(policy=secret, arbitrary=secret)
            s["native"]["safety"]["unrelated"] = secret
            s["oracle"].update(position=[secret], arbitrary=secret)
        data = self.timeout_summary(edit, components=lambda s: dict(unrelated=secret))
        encoded = json.dumps(data)
        self.assertNotIn("secret-content", encoded)
        self.assertNotIn("arbitrary", encoded)
        self.assertNotIn("position", encoded)
        self.assertEqual(set(data["failure_counts"]), set(gate.SettlingDiagnostics().data["failure_counts"]))

    def test_timeout_counter_saturation_and_no_success_diagnostics(self):
        diagnostics = gate.SettlingDiagnostics()
        limit = (1 << 32) - 1
        diagnostics.data["valid_acquisition_count"] = limit
        diagnostics.data["failure_counts"] = {k: limit for k in diagnostics.data["failure_counts"]}
        record = next(progress_records(PROGRESS_GOLDEN["cases"][0]))
        record["oracle"] = None
        diagnostics.observe(record, False, None, gate.SimulatorProgress(), 1, None)
        self.assertEqual(diagnostics.data["valid_acquisition_count"], limit)
        self.assertEqual(set(diagnostics.data["failure_counts"].values()), {limit})
        progress = gate.SimulatorProgress()
        progress.completed = True
        record = next(progress_records(PROGRESS_GOLDEN["cases"][0]))
        record["source_us"] = 10**10000
        diagnostics.observe(record, True, 1, progress, 1, {"gravity_z": 10**10000})
        self.assertTrue(diagnostics.data["timing_clamped"])
        self.assertEqual(diagnostics.data["longest_settling_streak_us"], (1 << 64) - 1)
        self.assertTrue(diagnostics.data["metrics"]["gravity_z"]["clamped"])
        self.assertLess(len(str(gate.SettlingTimeout(diagnostics.data))), 2500)
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            self.assertEqual(self.run_case(), 51)
        self.assertEqual((out.getvalue(), err.getvalue()), ("", ""))

    def test_timeout_without_acquisitions_reports_incomplete_window(self):
        clock_values = iter((100000000, 10100000000, 10100000000))
        with self.assertRaises(gate.SettlingTimeout) as rejected:
            gate.provision(lambda *args: {"accepted": True}, lambda: self.fail("deadline already reached"),
                           ("daemon", "body", "world"), lambda: next(clock_values))
        data = rejected.exception.data
        self.assertEqual(data["valid_acquisition_count"], 0)
        self.assertFalse(data["progress_window_completed"])
        self.assertIsNone(data["progress_window_completed_source_us"])
        self.assertFalse(data["settling_predicate_ever_true"])

    def test_timeout_entrypoint_emits_only_one_summary_without_traceback(self):
        # Execute the actual entrypoint with a failing run, without launching native I/O.
        tree = ast.parse(Path(gate.__file__).read_text())
        entrypoint = tree.body[-1]
        self.assertIsInstance(entrypoint, ast.If)
        def run():
            raise gate.SettlingTimeout(gate.SettlingDiagnostics().data)
        scope = dict(__name__="__main__", run=run, sys=sys, contextlib=contextlib,
                     AcquisitionError=gate.AcquisitionError, SettlingTimeout=gate.SettlingTimeout)
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            with self.assertRaises(SystemExit) as stopped:
                exec(compile(ast.Module(body=[entrypoint], type_ignores=[]), gate.__file__, "exec"), scope)
        self.assertEqual(stopped.exception.code, 1)
        self.assertEqual(out.getvalue(), "")
        self.assertEqual(len(err.getvalue().splitlines()), 1)
        self.assertNotIn("Traceback", err.getvalue())
        self.assertNotIn(str(Path(gate.__file__).parent), err.getvalue())
        self.assertLess(len(err.getvalue()), 2500)

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
        frames = iter((100500, *range(120000, 1120001, 100000)))
        now = [100000]
        sequence = [0]
        def sample():
            source = next(frames)
            now[0] = source + 1
            sequence[0] += 1
            return dict(source_us=source, daemon="d", body="b", world="w",
                        simulation_us=source, sequence=sequence[0],
                        native={"t_ns": (source-1000)*1000, "policy": "stand", "safety": {"fallen": False}},
                        oracle=dict(upright=True, yaw=0., linear_speed=0.,
                                    angular_speed=0., uncertainty=.000001))
        with patch.object(gate.time, "monotonic_ns", side_effect=lambda: now[0]*1000):
            result = gate.collect_standing(sample, ("d", "b", "w"), minimum_native_us=100000)
        self.assertEqual([s["source_us"] for s in result], list(range(120000, 1120001, 100000)))


class SimulatorProgressProof(unittest.TestCase):
    def test_reported_adjacent_reads_repeat_world_time_with_fresh_advancing_acquisition(self):
        case = next(c for c in PROGRESS_GOLDEN["cases"] if c["name"] == "adjacent-repeat-real-deltas")
        records = progress_records(case)
        previous, current = next(records), next(records)
        self.assertEqual((previous["simulation_us"], current["simulation_us"]), (340000, 340000))
        self.assertEqual(current["source_us"]-previous["source_us"], 25867)
        self.assertEqual((current["native"]["t_ns"]-previous["native"]["t_ns"])//1000, 26150)
        progress = gate.SimulatorProgress()
        for record in (previous, current):
            self.assertTrue(gate.same_control_frame(record["native"]["t_ns"], record["source_us"]))
            progress.observe(record, record["source_us"]+17065)
        self.assertFalse(progress.completed)  # Two fresh reads cannot finish startup proof.

    def test_shared_python_rust_vectors(self):
        for case in PROGRESS_GOLDEN["cases"]:
            with self.subTest(case=case["name"]):
                progress = gate.SimulatorProgress()
                accepted = True
                try:
                    for record in progress_records(case):
                        progress.observe(record, record["source_us"]+17065)
                except gate.AcquisitionError as error:
                    accepted = False
                    if "rejectedAtStep" in case:
                        self.assertEqual(record["sequence"]-1, case["rejectedAtStep"])
                        self.assertEqual(error.data["failed_conditions"],
                                         ["abs(window_simulation_delta_us - window_source_delta_us) <= 272000"]
                                         if case["failure"] == "phase" else ["simulation_us >= previous_simulation_us"])
                self.assertEqual(accepted, case["accepted"])
                if accepted:
                    self.assertEqual(progress.completed, case["completed"])

    def test_provisioning_accepts_jitter_and_rejects_slow_fast_or_incomplete_proof(self):
        for case in PROGRESS_GOLDEN["cases"]:
            with self.subTest(case=case["name"]):
                records = iter(progress_records(case))
                now = [80000]
                def sample():
                    record = next(records, None)
                    if record:
                        now[0] = record["source_us"]+17065
                    return record
                if case["accepted"] and case["completed"]:
                    gate.provision(lambda *args: {"accepted": True}, sample,
                                   ("daemon", "body", "world"), lambda: now[0]*1000)
                else:
                    # Second-window failure must also be exercised after initial proof.
                    if case["name"].startswith("second-window-"):
                        continue
                    with self.assertRaises(RuntimeError):
                        gate.provision(lambda *args: {"accepted": True}, sample,
                                       ("daemon", "body", "world"), lambda: now[0]*1000)

    def test_bounded_delta_and_window_rejection_diagnostics(self):
        case = next(c for c in PROGRESS_GOLDEN["cases"] if c["name"] == "slow")
        progress = gate.SimulatorProgress()
        with self.assertRaises(gate.AcquisitionError) as rejected:
            for record in progress_records(case):
                progress.observe(record, record["source_us"]+17065, enabled_us=80000)
        data = rejected.exception.data
        self.assertEqual((data["previous_source_us"], data["source_us"], data["source_delta_us"]),
                         (400000, 500000, 100000))
        self.assertEqual((data["previous_native_us"], data["native_us"], data["native_delta_us"]),
                         (399382, 499382, 100000))
        self.assertEqual(data["native_delta_ns"], 100000000)
        self.assertEqual((data["previous_simulation_us"], data["simulation_us"], data["simulation_delta_us"]),
                         (160000, 180000, 20000))
        self.assertEqual(data["sequence_delta"], 1)
        self.assertEqual((data["window_source_delta_us"], data["window_simulation_delta_us"]), (400000, 80000))
        self.assertEqual(data["failed_conditions"],
                         ["abs(window_simulation_delta_us - window_source_delta_us) <= 272000"])
        self.assertLess(len(str(rejected.exception)), 1200)

    def test_collector_retains_complete_bounded_window_and_validates_raw_samples(self):
        case = PROGRESS_GOLDEN["cases"][0]
        records = iter(progress_records(case))
        now = [80000]
        def sample():
            record = next(records)
            now[0] = record["source_us"]+17065
            return record
        with patch.object(gate.time, "monotonic_ns", side_effect=lambda: now[0]*1000):
            result = gate.collect_standing(sample, ("daemon", "body", "world"))
        self.assertEqual(len(result), 11)
        self.assertEqual(result[-1]["source_us"]-result[0]["source_us"], 1000000)
        # A bad raw read between retained 100 ms observations cannot be hidden.
        records = iter(progress_records(case))
        count = [0]
        original_sample = sample
        def bad_sample():
            record = original_sample()
            count[0] += 1
            if count[0] == 3:
                record["simulation_us"] = 119999  # regress below the previous raw tick
            return record
        with patch.object(gate.time, "monotonic_ns", side_effect=lambda: now[0]*1000):
            with self.assertRaises(gate.AcquisitionError):
                gate.collect_standing(bad_sample, ("daemon", "body", "world"))

    def test_thinning_cannot_create_a_stale_gap_in_retained_evidence(self):
        sources = iter((100000, 180000, *range(340000, 1140001, 100000)))
        now, sequence = [80000], [0]
        def sample():
            source = next(sources)
            now[0] = source+17065
            sequence[0] += 1
            return dict(source_us=source, simulation_us=source, sequence=sequence[0],
                        daemon="d", body="b", world="w", native={"t_ns": (source-618)*1000, "policy": "stand", "safety": {"fallen": False}},
                        oracle=dict(upright=True, yaw=0., linear_speed=0., angular_speed=0., uncertainty=.000001))
        with patch.object(gate.time, "monotonic_ns", side_effect=lambda: now[0]*1000):
            records = gate.collect_standing(sample, ("d", "b", "w"))
        self.assertLessEqual(len(records), 32)
        self.assertEqual([s["source_us"] for s in records[:3]], [100000, 180000, 340000])
        progress = gate.SimulatorProgress()
        for record in records:
            progress.observe(record, now[0])
        self.assertTrue(progress.completed)


if __name__ == "__main__":
    unittest.main()
