#!/usr/bin/env python3
"""Stage 9A Linux preparation/readiness. Never qualifies, enrolls or releases a body.

Writes only a new external work directory and a candidate report. Production
profile-v1.json is never edited. Requires an explicitly supplied Python 3.12.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tomllib
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "native/microduck/environment-v1.json"
PROFILE = ROOT / "native/microduck/profile-v1.json"


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def sha_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def environment_digest(root):
    """Exact qualification.rs BTreeMap<relative POSIX path, file SHA> semantics."""
    root = Path(root)
    files = {}
    def visit(directory):
        for path in directory.iterdir():
            if path.name == "__pycache__" or path.suffix == ".pyc":
                continue
            if path.is_symlink() and path.is_dir():
                raise RuntimeError("Symlinked Python environment directory unsupported")
            if path.is_dir():
                visit(path)
            elif path.is_file():
                relative = path.relative_to(root).as_posix()
                relative.encode("utf-8", "strict")
                files[relative] = sha_file(path)
            else:
                raise RuntimeError("Unsupported Python environment object")
    visit(root)
    if not files:
        raise RuntimeError("Empty Python environment")
    encoded = json.dumps(files, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def runtime_requirements(lock):
    """Only body-server/CPU inference roots and their locked dependency closure.

    No training, Torch/CUDA, viewer or BAM Python imports are used by body_server.
    Refuse unexpected resolution/markers rather than silently resolve new inputs.
    pip chooses the compatible wheel from this upstream-locked hash allowlist.
    """
    packages = {}
    for p in lock["package"]:
        packages.setdefault(p["name"], []).append(p)
    selected = {}
    visited = set()
    def visit(name, extras=()):
        key = name, tuple(extras)
        if key in visited:
            return
        visited.add(key)
        candidates = packages[name]
        if len(candidates) != 1:
            raise RuntimeError("Ambiguous locked runtime package: " + name)
        p = candidates[0]
        if p.get("source") != {"registry": "https://pypi.org/simple"}:
            raise RuntimeError("Unreviewed runtime source: " + name)
        selected[name] = p
        deps = list(p.get("dependencies", []))
        for extra in extras:
            deps += p.get("optional-dependencies", {})[extra]
        for dep in deps:
            if "marker" in dep:
                raise RuntimeError("Unreviewed runtime dependency marker: " + name)
            visit(dep["name"], dep.get("extra", []))
    for name in ("mujoco", "numpy", "onnxruntime"):
        visit(name)
    lines = []
    for name, p in sorted(selected.items()):
        # The upstream ORT entry also contains an unrelated bcrypt wheel. Never
        # allow a URL/hash whose distribution/version differs from this package.
        prefix = name.replace("-", "_") + "-" + p["version"] + "-"
        wheels = [w for w in p.get("wheels", []) if w["url"].rsplit("/", 1)[1].startswith(prefix)]
        if not wheels:
            raise RuntimeError("No exact wheel for " + name)
        hashes = sorted({w["hash"] for w in wheels})
        lines.append(name + "==" + p["version"] + " " + " ".join("--hash=" + h for h in hashes))
    return "\n".join(lines) + "\n", {n: p["version"] for n, p in sorted(selected.items())}


def reference_params(template, policies):
    """Pinned deploy template, only explicit reference slot selection added."""
    raw = Path(template).read_bytes()
    parsed = tomllib.loads(raw.decode())
    policy = parsed["policy"]
    slots = ("walk", "stand", "sitstand", "ground_pick", "kick_left", "kick_right", "roulade")
    if (policy.get("enabled") is not True or policy.get("mode") != "walk"
            or any(slot in policy for slot in slots) or raw.count(b"[policy]\n") != 1):
        raise RuntimeError("Pinned params template/reference recipe changed")
    values = dict(zip(("walk", "stand"), map(str, policies)))
    values.update({slot: "none" for slot in slots[2:]})
    insertion = "# Pastey reference-velocity-v1: exact upstream pair, other slots disabled.\n"
    insertion += "".join(k + " = " + json.dumps(v, ensure_ascii=False) + "\n" for k, v in values.items())
    return raw.replace(b"[policy]\n", b"[policy]\n" + insertion.encode())


def fetch_exact(url, path, digest):
    with urllib.request.urlopen(url, timeout=60) as response, Path(path).open("xb") as stream:
        shutil.copyfileobj(response, stream)
    if sha_file(path) != digest:
        raise RuntimeError("Exact upstream artifact SHA mismatch: " + url)


def namespace_command(python, mounts, args):
    command = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session",
               "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]
    # Same mounts as the owned production launcher; also expose the base Python
    # if the explicitly supplied interpreter is itself below host /tmp.
    for path in sorted(set(Path(p).absolute() for p in mounts)):
        if path.is_relative_to("/tmp") or path.is_relative_to("/private/tmp"):
            command += ["--ro-bind", str(path), str(path)]
    return command + [str(python), "-I", "-B", "-u", str(Path(__file__).resolve()), *args]


def child(mode, config_path):
    config = json.loads(Path(config_path).read_text())
    import numpy
    import mujoco
    import onnxruntime
    catalog = json.loads(CATALOG.read_text())
    actual = dict(python=platform.python_version(), mujoco=mujoco.__version__,
                  numpy=numpy.__version__, onnxruntime=onnxruntime.__version__)
    if (sys.version_info[:2] != (3, 12) or actual["mujoco"] != catalog["mujocoVersion"]
            or actual["numpy"] != catalog["numpyVersion"]
            or actual["onnxruntime"] != catalog["onnxRuntimeVersion"]):
        raise RuntimeError("Exact locked Python imports/version mismatch")
    runtimes = list((Path(onnxruntime.__file__).parent / "capi").glob("libonnxruntime.so.*"))
    if len(runtimes) != 1 or runtimes[0].resolve() != Path(config["ort"]).resolve():
        raise RuntimeError("Native ORT is not the exact imported wheel library")
    gate = module("supervisor", "microduck-gate-a.py")
    if mode == "--model-child":
        with contextlib.redirect_stdout(sys.stderr):
            native, _, world, body, digest = gate.build_simulator(config["rl"])
        if len(body.actuators) != 14 or body.index != 0 or len(world.bodies) != 1:
            raise RuntimeError("Expected single MicroDuck body/14 actuators missing")
        print(json.dumps(dict(imports=actual, modelSha256=digest,
                              scene=str(native.DEFAULT_SCENE), actuators=len(body.actuators))))
    else:
        identity = config["identity"]
        gate.sys.argv = ["owned-readiness", config["robotd"], config["rl"], config["params"],
                         json.dumps(config["policies"]), identity["controller"], identity["body"],
                         identity["world"], json.dumps(config["pins"]), identity["environment"],
                         identity["domain"], identity["body_ref"]]
        with contextlib.redirect_stdout(sys.stderr):
            gate.run(readiness_only=True)


def prepare(args, report):
    catalog = json.loads(CATALOG.read_text())
    profile = json.loads(PROFILE.read_text())
    owned = module("owned_prepare", "prepare-microduck-gate-b.py")
    source = owned.checked_source(args.native_source, catalog["upstream"])
    rl = owned.checked_source(args.rl_source, catalog["rlUpstream"])
    if (catalog["upstream"] != profile["upstream"] or catalog["rlUpstream"] != profile["rlUpstream"]
            or owned.PIN != profile["upstream"] or owned.RL_PIN != profile["rlUpstream"]):
        raise RuntimeError("Production/catalog/source pins disagree")
    if (sha_file(source / catalog["paramsTemplate"]) != catalog["paramsTemplateSha256"]
            or sha_file(rl / "uv.lock") != catalog["rlLockSha256"]):
        raise RuntimeError("Exact source resource bytes mismatch")
    report["sourceVerification"] = dict(native=str(source), rl=str(rl),
                                       upstream=catalog["upstream"], rlUpstream=catalog["rlUpstream"], clean=True)
    requirements, versions = runtime_requirements(tomllib.loads((rl / "uv.lock").read_text()))
    report["lockedRuntimeVersions"] = versions
    blockers = []
    if sys.platform != "linux":
        blockers.append("Linux mount/PID/network namespaces unavailable on " + platform.platform())
    for tool in ("bwrap", "cargo", "git"):
        if shutil.which(tool) is None:
            blockers.append("Required host tool absent: " + tool)
    if not args.python or not args.python.is_file():
        blockers.append("Explicit Python 3.12 executable unavailable")
    if blockers:
        raise RuntimeError("; ".join(blockers))
    report["linuxEnvironment"] = dict(kernel=platform.release(), architecture=platform.machine(),
                                      libc=platform.libc_ver(), osRelease=Path("/etc/os-release").read_text(),
                                      tools={tool: subprocess.check_output([tool, "--version"], text=True).strip()
                                             for tool in ("bwrap", "rustc", "cargo")})
    base_python = args.python.absolute()  # retain venv semantics, hash canonical target separately
    version = subprocess.check_output([str(base_python), "-I", "-B", "-c",
                                       "import sys; print('.'.join(map(str,sys.version_info[:3])))"], text=True).strip()
    if not version.startswith("3.12."):
        raise RuntimeError("Python 3.12 required, found " + version)
    if not args.work or args.work.exists() or args.work.is_relative_to(ROOT):
        raise RuntimeError("A new external absolute --work directory is required")
    work = args.work
    work.mkdir(mode=0o700)
    report["work"] = str(work)
    clean_env = os.environ.copy()
    for key in ("PYTHONPATH", "PYTHONHOME", "LD_PRELOAD", "LD_LIBRARY_PATH"):
        clean_env.pop(key, None)
    subprocess.run([str(base_python), "-I", "-B", "-m", "venv", str(work / "venv")], check=True, env=clean_env)
    python = work / "venv/bin/python"
    req = work / "runtime-requirements.txt"
    req.write_text(requirements)
    subprocess.run([str(python), "-I", "-B", "-m", "pip", "--isolated", "install", "--require-hashes",
                    "--only-binary=:all:", "--no-deps", "--index-url", "https://pypi.org/simple",
                    "-r", str(req)], check=True, env=clean_env)
    report["pythonPreparation"] = dict(version=version, requirementsSha256=sha_file(req), result="PASS")
    # Existing owned builder archives exact clean sources, applies the accepted
    # overlay and builds with Cargo.lock; it never adopts a supplied robotd.
    subprocess.run([str(python), "-I", "-B", str(ROOT / "scripts/prepare-microduck-gate-b.py"),
                    str(source), str(rl), str(work / "package")], check=True, env=clean_env)
    report["overlayAndNativeBuild"] = "PASS (Linux cargo build --locked --release -p robotd)"
    report["overlaySha256"] = {p: sha_file(ROOT / p) for p in (
        "native/microduck/upstream.patch", "native/microduck/overlay/duck-ipc-proto/src/task_authority.rs",
        "native/microduck/overlay/robotd/src/task_authority.rs")}
    package = work / "package"
    policies = []
    base_url = "https://huggingface.co/" + catalog["policyRepository"] + "/resolve/" + catalog["policyRevision"] + "/"
    fetch_exact(base_url + "manifest.json", work / "manifest.json", catalog["policyManifestSha256"])
    manifest = json.loads((work / "manifest.json").read_text())
    if manifest.get("obs_len") != 61 or manifest.get("action_len") != 14:
        raise RuntimeError("Upstream policy-set tensor contract mismatch")
    for item in catalog["policies"]:
        if not any(p["file"] == item["file"] and p["kind"] == "perpetual" for p in manifest["policies"]):
            raise RuntimeError("Exact walk/stand artifact missing from immutable official set")
        path = work / item["file"]
        fetch_exact(base_url + item["file"], path, item["sha256"])
        policies.append(path)
    params = work / "reference-robotd.toml"
    params.write_bytes(reference_params(source / catalog["paramsTemplate"], policies))
    libraries = list((work / "venv/lib/python3.12/site-packages/onnxruntime/capi").glob("libonnxruntime.so.*"))
    if len(libraries) != 1:
        raise RuntimeError("Exactly one Linux ORT wheel runtime library required")
    ort = libraries[0]
    pins = dict(profile, pythonEnvironmentSha256=environment_digest(work / "venv"),
                pythonExecutableSha256=sha_file(python.resolve()), onnxRuntimeSha256=sha_file(ort),
                paramsSha256=sha_file(params), policySha256=[sha_file(p) for p in policies])
    config = dict(robotd=str(package / "microduck/target/release/robotd"), rl=str(package / "rl"),
                  ort=str(ort), params=str(params), policies=list(map(str, policies)), pins=pins,
                  identity={k: str(uuid.uuid4()) for k in ("controller", "body", "world", "environment", "body_ref")})
    # Valid versioned wire IDs are required by the native protocol.
    config["identity"].update(environment="physical-environment:v1:"+config["identity"]["environment"],
                              body_ref="physical-body:v1:"+config["identity"]["body_ref"], domain="body-motion")
    config_path = work / "readiness-config.json"
    config_path.write_text(json.dumps(config))
    mounts = [work, ROOT, base_python.resolve().parent.parent]
    clean_env.update(ORT_DYLIB_PATH=str(ort), PASTEY_PARENT_NAMESPACES=json.dumps(
        [os.readlink("/proc/self/ns/" + n) for n in ("mnt", "pid", "net")]))
    def probe(mode):
        result = subprocess.check_output(namespace_command(python, mounts, [mode, str(config_path)]),
                                         text=True, env=clean_env, timeout=40)
        return json.loads(result)
    model = probe("--model-child")
    report["modelConstruction"] = model
    # Core rebuilds into a fresh package path on every launch. Verify that MJB
    # identity does not accidentally pin this discovery directory's spelling.
    shutil.copytree(package / "rl", work / "model-reproduction" / "rl")
    config["rl"] = str(work / "model-reproduction" / "rl")
    config_path.write_text(json.dumps(config))
    reproduced_model = probe("--model-child")
    report["modelReproduction"] = reproduced_model
    if reproduced_model["modelSha256"] != model["modelSha256"] or reproduced_model["imports"] != model["imports"]:
        raise RuntimeError("Compiled model identity changed across fresh source paths")
    config["rl"] = str(package / "rl")
    pins.update(mujocoVersion=model["imports"]["mujoco"], compiledModelSha256=model["modelSha256"])
    config_path.write_text(json.dumps(config))
    readiness = probe("--readiness-child")
    parent_ns = json.loads(clean_env["PASTEY_PARENT_NAMESPACES"])
    if (readiness.get("stage") != "9A" or readiness.get("readiness") != "READY"
            or readiness.get("qualification") is not False or readiness.get("release") is not False
            or readiness.get("modelSha256") != pins["compiledModelSha256"]
            or readiness.get("mujocoVersion") != pins["mujocoVersion"]
            or len(readiness.get("namespaces", [])) != 3
            or any(a == b for a, b in zip(readiness["namespaces"], parent_ns))):
        raise RuntimeError("Real isolated readiness report mismatch")
    after = dict(pythonEnvironmentSha256=environment_digest(work / "venv"),
                 pythonExecutableSha256=sha_file(python.resolve()), onnxRuntimeSha256=sha_file(ort),
                 paramsSha256=sha_file(params), policySha256=[sha_file(p) for p in policies])
    if any(pins[k] != v for k, v in after.items()):
        raise RuntimeError("Runtime artifacts changed during readiness")
    owned.checked_source(source, catalog["upstream"])
    owned.checked_source(rl, catalog["rlUpstream"])
    pins["state"] = "READY_FOR_QUALIFICATION"
    report.update(readiness="READY", candidateProfilePins=pins, runtimeReadiness=readiness,
                  installation=config, robotdSha256=sha_file(config["robotd"]),
                  productionPinsCommitted=False,
                  review="Independently review this exact Linux run before editing the compiled production profile.")


def main():
    if len(sys.argv) == 3 and sys.argv[1] in ("--model-child", "--readiness-child"):
        child(*sys.argv[1:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-source", type=Path, required=True)
    parser.add_argument("--rl-source", type=Path, required=True)
    parser.add_argument("--python", type=Path, help="Explicit host Python 3.12")
    parser.add_argument("--work", type=Path, help="New external absolute work directory")
    parser.add_argument("--report", type=Path, required=True, help="New external JSON report; never a qualification record")
    args = parser.parse_args()
    for key in ("native_source", "rl_source", "python", "work", "report"):
        path = getattr(args, key)
        if path is not None:
            setattr(args, key, path.absolute())
    if args.report.exists() or args.report.is_relative_to(ROOT):
        parser.error("--report must be a new external path")
    report = dict(stage="9A", readiness="BLOCKED", qualification=False, release=False,
                  host=platform.platform(), architecture=platform.machine(), productionPinsCommitted=False)
    try:
        with contextlib.redirect_stdout(sys.stderr):
            prepare(args, report)
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError) as error:
        report["blocker"] = str(error)
    with args.report.open("x") as stream:
        stream.write(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(dict(readiness=report["readiness"], report=str(args.report), blocker=report.get("blocker"))))
    if report["readiness"] != "READY":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
