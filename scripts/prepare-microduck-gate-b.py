#!/usr/bin/env python3
"""Owned exact native rebuild. Called by Core, never adopts a supplied binary.

A clean revision-pinned source tree is exported to a new private directory, the
compiled Pastey overlay is applied, and Cargo.lock is mandatory. No fetch/policy
selection, qualification, authority or release record is performed here.
"""
from pathlib import Path
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]
PIN = "a9ec4b2079ef8ee7904014089c885bb07d57d63c"
RL_PIN = "cb70b792312d559a4da09064d92009079671815f"


def checked_source(path, pin):
    path = Path(path).resolve()
    def git(*args):
        return subprocess.check_output(["git", "-C", str(path), *args], text=True).strip()
    if git("rev-parse", "HEAD") != pin or git("status", "--porcelain", "--untracked-files=all"):
        raise RuntimeError("exact clean pinned source required: " + pin)
    if git("submodule", "status"):
        raise RuntimeError("unqualified submodule inputs")
    return path


def prepare(source, rl, output):
    source = checked_source(source, PIN)
    rl = checked_source(rl, RL_PIN)
    output = Path(output)
    output.mkdir(mode=0o700)  # existing package never reused
    repo = output / "microduck"
    repo.mkdir()
    archive = subprocess.check_output(["git", "-C", str(source), "archive", PIN])
    import io
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(repo, filter="data")
    subprocess.run(["git", "-C", str(repo), "apply", "--unidiff-zero", str(ROOT / "native/microduck/upstream.patch")], check=True)
    for name in ("duck-ipc-proto/src/task_authority.rs", "robotd/src/task_authority.rs"):
        (repo / name).write_bytes((ROOT / "native/microduck/overlay" / name).read_bytes())
    rl_copy = output / "rl"
    rl_copy.mkdir()
    archive = subprocess.check_output(["git", "-C", str(rl), "archive", RL_PIN])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(rl_copy, filter="data")
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "robotd"], cwd=repo, check=True)
    return repo


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("owned launcher arguments required")
    prepare(*sys.argv[1:])
