#!/usr/bin/env python3
"""Apply the narrow Stage 8 overlay to a clean, exactly pinned MicroDuck checkout."""
import argparse
from pathlib import Path
import subprocess

PIN = "a9ec4b2079ef8ee7904014089c885bb07d57d63c"
ROOT = Path(__file__).resolve().parents[1]
FILES = ("duck-ipc-proto/src/task_authority.rs", "robotd/src/task_authority.rs")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    repo = parser.parse_args().checkout.resolve()
    def git(*args):
        return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()
    if git("rev-parse", "HEAD") != PIN or git("status", "--porcelain"):
        raise SystemExit("requires a clean checkout at " + PIN)
    contents = [(repo / name, (ROOT / "native/microduck/overlay" / name).read_bytes()) for name in FILES]
    if any(path.exists() for path, _ in contents):
        raise SystemExit("overlay destination already exists")
    patch = ROOT / "native/microduck/upstream.patch"
    git("apply", "--check", "--unidiff-zero", str(patch))
    git("apply", "--unidiff-zero", str(patch))
    for path, content in contents:
        path.write_bytes(content)
    print("Applied Stage 8 source overlay to", repo)
    print("No build, process launch, qualification or release was performed.")

if __name__ == "__main__":
    main()
