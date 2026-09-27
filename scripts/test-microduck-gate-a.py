"""Deterministic supervisor provisioning tests; no native integration claim."""
import copy
import importlib.util
from pathlib import Path
import unittest

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
                     native={"t_ns": now[0] * 1000, "policy": "stand", "safety": {"fallen": False}},
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


if __name__ == "__main__":
    unittest.main()
