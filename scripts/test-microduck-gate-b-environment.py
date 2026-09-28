"""Stage 9A preparation regressions. Never real simulator/qualification evidence."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


env = load("environment", "prepare-microduck-gate-b-environment.py")
gate = load("supervisor", "microduck-gate-a.py")
GOLDEN = json.loads((Path(__file__).parent / "fixtures/microduck-environment-digest-v1.json").read_text())


def digest_fixture(root):
    root.mkdir()
    for relative, text in GOLDEN["files"].items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())
    for relative, target in GOLDEN["fileSymlinks"].items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.symlink_to(target)


class EnvironmentPreparation(unittest.TestCase):
    def test_digest_matches_independent_rust_golden(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "A.txt").write_bytes(b"alpha\n")
            (root / "é.txt").write_bytes(b"beta")
            (root / "bin").mkdir()
            (root / "bin/python").symlink_to("../A.txt")
            (root / "__pycache__").mkdir()
            (root / "__pycache__/ignored").write_text("ignored")
            (root / "ignored.pyc").write_text("ignored")
            self.assertEqual(env.environment_digest(root),
                             "f5e3ddcd49df7a6204739b6f02e3427a231cd6be882c8fd159df7bf264151168")

    def test_standard_venv_alias_shared_golden_and_identity_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "venv"
            digest_fixture(root)
            self.assertEqual(env.environment_digest(root), GOLDEN["digests"]["withoutDirectoryAlias"])
            alias = root / "lib64"
            alias.symlink_to("lib", target_is_directory=True)
            with patch.object(env, "sha_file", wraps=env.sha_file) as hashed:
                self.assertEqual(env.environment_digest(root), GOLDEN["digests"]["lib64ToLib"])
                canonical_root = root.resolve()
                self.assertEqual(sum(c.args[0] == canonical_root / "lib/package.py" for c in hashed.call_args_list), 1)
                self.assertFalse(any(c.args[0].is_relative_to(canonical_root / "lib64") for c in hashed.call_args_list))
            alias.unlink()
            self.assertEqual(env.environment_digest(root), GOLDEN["digests"]["withoutDirectoryAlias"])
            alias.symlink_to("other-lib", target_is_directory=True)
            self.assertEqual(env.environment_digest(root), GOLDEN["digests"]["lib64ToOtherLib"])
            alias.unlink()
            alias.symlink_to("./lib", target_is_directory=True)
            self.assertEqual(env.environment_digest(root), GOLDEN["digests"]["lib64ToDotLib"])
            # Canonically resolve a relative directory-alias chain as well.
            alias.unlink()
            alias.symlink_to("current", target_is_directory=True)
            (root / "current").symlink_to("lib", target_is_directory=True)
            self.assertIsInstance(env.environment_digest(root), str)

    def test_directory_alias_escape_broken_links_and_graph_cycles_rejected(self):
        for fault in ("absolute-inside", "absolute-outside", "relative-outside", "broken",
                      "self-cycle", "link-cycle", "ancestor-cycle", "sibling-cycle", "cache-cycle"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "venv"
                digest_fixture(root)
                outside = Path(directory) / "outside"
                outside.mkdir()
                alias = root / "lib64"
                if fault == "absolute-inside":
                    alias.symlink_to(root / "lib")
                elif fault == "absolute-outside":
                    alias.symlink_to(outside)
                elif fault == "relative-outside":
                    alias.symlink_to("../outside")
                elif fault == "broken":
                    alias.symlink_to("missing")
                elif fault == "self-cycle":
                    alias.symlink_to("lib64")
                elif fault == "link-cycle":
                    alias.symlink_to("current")
                    (root / "current").symlink_to("lib64")
                elif fault == "ancestor-cycle":
                    (root / "lib/back").symlink_to("..")
                elif fault == "sibling-cycle":
                    (root / "lib/to-other").symlink_to("../other-lib")
                    (root / "other-lib/to-lib").symlink_to("../lib")
                else:
                    (root / "__pycache__/back").symlink_to("..")
                with self.assertRaises(RuntimeError):
                    env.environment_digest(root)

    def test_external_file_symlink_still_hashes_target_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "venv"
            digest_fixture(root)
            external = Path(directory) / "python"
            external.write_bytes(b"executable")
            (root / "bin/external-python").symlink_to(external)
            before = env.environment_digest(root)
            external.write_bytes(b"changed-executable")
            self.assertNotEqual(before, env.environment_digest(root))

    def test_runtime_closure_uses_lock_hashes_extras_and_no_training(self):
        def package(name, dependencies=()):
            return dict(name=name, version="1.0", source={"registry": "https://pypi.org/simple"},
                        dependencies=list(dependencies), wheels=[dict(url="https://example.invalid/"+name.replace("-", "_")+"-1.0-py3-none-any.whl",
                                                                     hash="sha256:"+"a"*64)])
        packages = [package("mujoco", [dict(name="helper", extra=["path"])]),
                    package("numpy"), package("onnxruntime"), package("helper"), package("path-helper"), package("torch")]
        packages[3]["optional-dependencies"] = {"path": [dict(name="path-helper")]}
        packages[2]["wheels"].append(dict(url="https://example.invalid/bcrypt-1.0-py3-none-any.whl", hash="sha256:"+"b"*64))
        text, versions = env.runtime_requirements(dict(package=packages))
        self.assertEqual(set(versions), {"mujoco", "numpy", "onnxruntime", "helper", "path-helper"})
        self.assertNotIn("b"*64, text)
        self.assertNotIn("torch", text)
        packages[0]["dependencies"][0]["marker"] = "some-new-marker"
        with self.assertRaisesRegex(RuntimeError, "marker"):
            env.runtime_requirements(dict(package=packages))

    def test_params_preserve_controller_and_safety(self):
        template = b'[control]\nhz = 50\n[safety]\nlimit = 1.0\n[policy]\nenabled = true\nmode = "walk"\n'
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "template.toml"
            path.write_bytes(template)
            generated = env.reference_params(path, [Path("/external/walk.onnx"), Path("/external/stand.onnx")])
            self.assertEqual(generated.split(b"[policy]")[0], template.split(b"[policy]")[0])
            self.assertTrue(generated.endswith(b'enabled = true\nmode = "walk"\n'))
            self.assertIn(b'roulade = "none"', generated)
            path.write_bytes(template + b'walk = "replacement.onnx"\n')
            with self.assertRaisesRegex(RuntimeError, "recipe changed"):
                env.reference_params(path, [Path("a"), Path("b")])

    def test_bad_download_never_accepted(self):
        import io
        with tempfile.TemporaryDirectory() as directory, patch.object(env.urllib.request, "urlopen", return_value=io.BytesIO(b"wrong")):
            with self.assertRaisesRegex(RuntimeError, "SHA mismatch"):
                env.fetch_exact("https://example.invalid/exact", Path(directory)/"policy", "a"*64)

    def readiness(self, *, missing_stand=False, stale=False, paused=False, unfenced=False):
        identity = dict(controller="controller", body="body", world="world")
        samples = [dict(daemon="controller", body="body", world="world", source_us=t,
                        simulation_us=t if not paused else 100_000, sequence=i+1, native={"t_ns": t*1000+1000})
                   for i, t in enumerate((100_000, 200_000, 300_000))]
        status = dict(identity=identity, protocol="microduck-task-v1", profile="reference-velocity-v1", fenced=not unfenced,
                      accepted=True, reason="not_installed", installed=None, action=None,
                      high_water_epoch=0, sequence=0, consumed_sequence=0)
        calls = []
        def rpc(method, request):
            calls.append((method, copy.deepcopy(request)))
            return status
        subscribed = dict(walk="walk.onnx", stand=None if missing_stand else "stand.onnx")
        with patch.object(gate, "collect_standing", return_value=samples), \
                patch.object(gate.time, "monotonic_ns", return_value=(600_000 if stale else 301_000)*1000), \
                patch.object(gate.os, "readlink", side_effect=lambda p: p):
            result = gate.readiness_report(rpc, lambda: None, identity, subscribed,
                                          ["/tmp/walk.onnx", "/tmp/stand.onnx"], "model", "engine")
        self.assertEqual(calls, [("robot.task", dict(kind="status", protocol="microduck-task-v1"))])
        self.assertEqual(result["stage"], "9A")
        self.assertIs(result["qualification"], False)
        self.assertIs(result["release"], False)
        self.assertNotIn("gate_b", result)
        self.assertNotIn("provisioned", result)
        return result

    def test_readiness_only_status_no_install_action_or_qualification(self):
        self.readiness()

    def test_missing_policy_stale_paused_or_unfenced_denies_readiness(self):
        for fault in ("missing_stand", "stale", "paused", "unfenced"):
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                self.readiness(**{fault: True})

    def test_catalog_is_not_a_partial_production_profile(self):
        catalog = json.loads(env.CATALOG.read_text())
        profile = json.loads(env.PROFILE.read_text())
        self.assertEqual(profile["state"], "PENDING_ENVIRONMENT")
        self.assertIsNone(profile["policySha256"])
        self.assertEqual(catalog["upstream"], profile["upstream"])
        self.assertEqual(catalog["rlUpstream"], profile["rlUpstream"])
        self.assertEqual([p["slot"] for p in catalog["policies"]], ["walk", "stand"])
        self.assertNotIn("state", catalog)


if __name__ == "__main__":
    unittest.main()
