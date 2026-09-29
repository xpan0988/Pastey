"""Owned output regression using real Cargo and synthetic exported sources.

Source verification/archive/overlay application are mocked; this is not a
MicroDuck build or simulator qualification test.
"""
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "prepare", Path(__file__).with_name("prepare-microduck-gate-b.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def archive(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        for name, text in files.items():
            data = text.encode()
            item = tarfile.TarInfo(name)
            item.size = len(data)
            tar.addfile(item, io.BytesIO(data))
    return buffer.getvalue()


class OwnedOutput(unittest.TestCase):
    def test_ambient_target_cannot_redirect_output_or_supply_robotd(self):
        native = archive({
            "Cargo.toml": '[package]\nname="robotd"\nversion="0.0.0"\nedition="2021"\n',
            "Cargo.lock": 'version = 4\n[[package]]\nname = "robotd"\nversion = "0.0.0"\n',
            "src/main.rs": 'fn main() { println!("owned-build"); }\n',
            "duck-ipc-proto/src/lib.rs": "",
            "robotd/src/main.rs": "",
        })
        rl = archive({"README.md": "synthetic RL export"})
        real_run = subprocess.run

        def run(command, **kwargs):
            if command[0] == "git":
                self.assertEqual(command[3], "apply")
                return subprocess.CompletedProcess(command, 0)
            self.assertEqual(command[0], "cargo")
            return real_run(command, **kwargs)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            external = root / "host-target"
            sentinel = external / "release/robotd"
            sentinel.parent.mkdir(parents=True)
            sentinel.write_text("external artifact must not be used or overwritten")
            before = sentinel.read_bytes()
            # Relative output also needs an absolute --target-dir: Cargo runs
            # with cwd set to the newly exported source tree.
            for relative in (False, True):
                with self.subTest(relative=relative):
                    output = root / ("relative-package" if relative else "absolute-package")
                    argument = Path(os.path.relpath(output)) if relative else output
                    with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(external)}), \
                            patch.object(prepare, "checked_source", side_effect=lambda path, pin: Path(path)), \
                            patch.object(prepare.subprocess, "check_output", side_effect=[native, rl]), \
                            patch.object(prepare.subprocess, "run", side_effect=run):
                        repo = prepare.prepare(root / "native", root / "rl", argument)
                    self.assertEqual(repo.resolve(), output / "microduck")
                    # Same fixed artifact path consumed by launch_native;
                    # deliberately do not discover/fall back to the host target.
                    robotd = output / "microduck/target/release/robotd"
                    self.assertEqual(real_run([str(robotd)], check=True, capture_output=True,
                                              text=True).stdout, "owned-build\n")
                    self.assertEqual(sentinel.read_bytes(), before)
                    self.assertEqual(sorted(p.relative_to(external).as_posix()
                                            for p in external.rglob("*")), ["release", "release/robotd"])


if __name__ == "__main__":
    unittest.main()
