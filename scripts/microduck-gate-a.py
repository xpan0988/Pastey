#!/usr/bin/env python3
"""Owned Linux simulation supervisor, separate Gate A and native Gate B modes.

Owned Rust launches and Stage 9A readiness use fresh bwrap mount/PID/network namespaces.
The pipe is private; it is not an enrollment/observation HTTP or socket service.
All simulator observations are read-only instrumentation of upstream World/Body.
No policy, Safety, RobotIo, actuator, tensor or physics implementation is changed.
"""
import collections
import contextlib
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import select
import subprocess
import sys
import threading
import time

OUTPUT = sys.stdout
MAX_LINE = 65536


def emit(value):
    OUTPUT.write(json.dumps(value, allow_nan=False, separators=(",", ":")) + "\n")
    OUTPUT.flush()


def provision(rpc, next_sample, identities, clock=time.monotonic_ns):
    """One-shot simulation preparation before the launcher can accept a run.

    No task identity or authority exists here. ACK is not a measurement. All
    proof samples must have been acquired after ACK, with continuous clocks.
    """
    deadline = clock() // 1000 + 10_000_000
    if rpc("robot.enable", {"on": True, "toggle": False}).get("accepted") is not True:
        raise RuntimeError("simulation provisioning enable refused")
    enabled_us = clock() // 1000
    head = None
    settled_since = None
    while clock() // 1000 < deadline:
        observation = next_sample()
        now = clock() // 1000
        if observation is None:
            raise RuntimeError("provisioning observation lost")
        if tuple(observation[k] for k in ("daemon", "body", "world")) != identities:
            raise RuntimeError("provisioning incarnation replaced")
        source, sim, seq = (observation[k] for k in ("source_us", "simulation_us", "sequence"))
        # Subscription frames already queued when the ACK arrived cannot prove
        # preparation. Discard only this bounded pre-ACK tail before first proof.
        if head is None and type(source) is int and 0 <= now - source < 200_000 and source <= enabled_us:
            continue
        native = observation["native"]
        tick = native.get("t_ns")
        if (type(source) is not int or type(sim) is not int or type(seq) is not int
                or type(tick) is not int or source <= enabled_us or sim <= 0 or seq <= 0
                or not source <= tick // 1000 < source + 20_000
                or not source <= now < source + 200_000):
            raise RuntimeError("stale/unproved provisioning acquisition")
        if head is not None:
            old_source, old_sim, old_seq, old_tick = head
            delta = source - old_source
            if (not 0 < delta < 200_000 or seq <= old_seq or tick <= old_tick
                    or not delta // 2 <= sim - old_sim <= delta * 2 + 20_000
                    or sim <= old_sim):
                raise RuntimeError("provisioning source gap/reset or paused simulator")
        head = source, sim, seq, tick
        oracle = observation.get("oracle")
        values = None if oracle is None else [oracle.get(k) for k in
                    ("yaw", "linear_speed", "angular_speed", "uncertainty")]
        valid = (values is not None and all(type(v) in (int, float) and math.isfinite(v) for v in values)
                 and oracle.get("upright") is True and abs(values[0]) <= 0.000001
                 and 0 <= values[1] <= 0.02 and 0 <= values[2] <= 0.1
                 and 0 <= values[3] <= 0.001
                 and native.get("policy") in ("stand", "walk")
                 and native.get("safety", {}).get("fallen") is False)
        settled_since = (source if settled_since is None else settled_since) if valid else None
        if settled_since is not None and source - settled_since >= 200_000:
            return  # Only new continuous settled measurements seal preparation.
    raise RuntimeError("simulation provisioning bring-up timeout")




def artifact_snapshot_digest(params, assets):
    return hashlib.sha256(b"".join(Path(p).read_bytes() for p in [params]+assets)).hexdigest()


def snapshot_native_artifacts(params, assets_raw):
    import re
    import tomllib
    assets = json.loads(assets_raw)
    text = Path(params).read_text()
    parsed = tomllib.loads(text)
    policy = parsed.get("policy", {})
    if policy.get("enabled") is not True or policy.get("mode", "walk") != "walk":
        raise RuntimeError("exact reference policy must be explicitly enabled walk mode")
    if [str(Path(policy.get(k, "")).resolve()) for k in ("walk", "stand")] != [str(Path(p).resolve()) for p in assets]:
        raise RuntimeError("parameter policy locators do not match exact artifact manifest")
    if any(policy.get(k) != "none" for k in ("sitstand", "ground_pick", "kick_left", "kick_right", "roulade")):
        raise RuntimeError("other skills/postures must be explicitly disabled")
    # Only locator strings are rewritten, once, inside the owned namespace.
    # Every controller/Safety/physics parameter byte remains unchanged.
    owned = []
    for slot, source in zip(("walk", "stand"), assets):
        target = Path("/tmp") / (slot + ".onnx")
        target.write_bytes(Path(source).read_bytes())
        owned.append(str(target))
    lines = text.splitlines(keepends=True)
    in_policy = False
    replacements = 0
    for i, line in enumerate(lines):
        if line.lstrip().startswith("["):
            in_policy = line.strip() == "[policy]"
        if in_policy:
            m = re.match(r"^(\s*)(walk|stand)\s*=.*?(\r?\n)?$", line)
            if m:
                lines[i] = m[1] + m[2] + " = " + json.dumps(owned[0 if m[2]=="walk" else 1]) + "\n"
                replacements += 1
    if replacements != 2:
        raise RuntimeError("unsupported policy locator syntax")
    target = Path("/tmp/robotd.toml")
    target.write_text("".join(lines))
    return str(target), json.dumps(owned)


def check_native_artifacts(pins, params, assets):
    if len(assets) != 2:
        raise RuntimeError("exact walk/stand artifacts required")
    sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
    if sha(params) != pins["paramsSha256"] or [sha(p) for p in assets] != pins["policySha256"]:
        raise RuntimeError("reference parameter/policy artifact changed")
    ort = os.environ.get("ORT_DYLIB_PATH")
    if ort is None or sha(ort) != pins["onnxRuntimeSha256"]:
        raise RuntimeError("exact native ONNX Runtime mismatch")



def collect_standing(next_sample, identities, dwell_us=200_000):
    result = []
    until = time.monotonic_ns() // 1000 + 10_000_000
    while time.monotonic_ns() // 1000 < until:
        try:
            s = next_sample()
        except RuntimeError as error:
            if str(error) != "stale native acquisition":
                raise
            result = []
            continue
        o = s.get("oracle")
        ready = o and o["upright"] and abs(o["yaw"]) <= 1e-6 and o["linear_speed"] <= .02 and o["angular_speed"] <= .1 and o["uncertainty"] <= .001
        if (s["daemon"], s["body"], s["world"]) != identities:
            raise RuntimeError("qualification incarnation changed")
        result = (result + [s])[-32:] if ready else []
        if len(result) >= 3 and result[-1]["source_us"] - result[0]["source_us"] >= dwell_us:
            return result
    raise RuntimeError("measured standing qualification unavailable")


def native_probes(rpc, next_sample, process, identity, sleep=time.sleep):
    import signal
    import uuid
    transcript = []
    expiry_witnesses = []
    expiry_moves = []
    def task(request):
        r = rpc("robot.task", request)
        if r.get("identity") != identity or r.get("protocol") != "microduck-task-v1" or r.get("profile") != "reference-velocity-v1":
            raise RuntimeError("native probe identity/protocol mismatch")
        return r
    def status():
        return task(dict(kind="status", protocol="microduck-task-v1"))
    def pair(epoch, lease_us, action_us):
        now = status()["native_us"]
        install = dict(protocol="microduck-task-v1", profile="reference-velocity-v1", identity=identity,
            domain=identity["domain"], session="physical-session:v1:"+str(uuid.uuid4()), epoch=epoch,
            request="physical-request:v1:"+str(uuid.uuid4()), lease_deadline_us=now+lease_us)
        ir = task(dict(kind="install", descriptor=install))
        action = dict(install=install, action="physical-action:v1:"+str(uuid.uuid4()),
            payload_digest=hashlib.sha256(b"qualification-reference-velocity").hexdigest(), deadline_us=now+action_us)
        ar = task(dict(kind="admit", descriptor=action))
        move = dict(action=action, request="physical-request:v1:"+str(uuid.uuid4()), sequence=1, twist=[.05,0,0])
        mr = task(dict(kind="move", descriptor=move))
        if not all(r.get("accepted") for r in (ir, ar, mr)):
            raise RuntimeError("native mechanism unavailable")
        return install, move, (ir, ar, mr)
    identities = tuple(identity[k] for k in ("controller", "body", "world"))
    reference_trace = [collect_standing(next_sample, identities)[-1]]
    install, move, initial = pair(1, 2_000_000, 1_000_000)
    transcript.extend(initial)
    last_move = initial[-1]
    for sequence in range(2, 21):
        sleep(.05)
        move["sequence"] = sequence
        last_move = task(dict(kind="move", descriptor=move))
        if not last_move.get("accepted"):
            raise RuntimeError("reference qualification refresh failed")
        reference_trace.append(next_sample())
    transcript.append(last_move)
    sleep(max(0, (move["action"]["deadline_us"] - time.monotonic_ns() // 1000) / 1_000_000))
    transcript.append(task(dict(kind="fence", descriptor=dict(install=install, next_epoch=2,
        request="physical-request:v1:"+str(uuid.uuid4())))))
    reference_trace.extend(collect_standing(next_sample, identities, 500_000))
    for epoch, lease, duration, delay, pause in [(3,1_000_000,600_000,.23,False),
            (4,500_000,120_000,.15,False), (5,120_000,120_000,.15,False),
            (6,500_000,120_000,.30,True)]:
        install, move, setup = pair(epoch, lease, duration)
        expiry_moves.append(setup[2])
        if pause:
            os.kill(process.pid, signal.SIGSTOP)
        try:
            sleep(delay)
        finally:
            if pause:
                os.kill(process.pid, signal.SIGCONT)
        # Read the loop's emitted state BEFORE any robot.task request can
        # lazily expire authority. Requested twist is mechanism diagnostics,
        # never measured displacement/rest or a consequence witness.
        cutoff = (setup[2]["native_us"] + 200_000 if epoch == 3 else
                  install["lease_deadline_us"] if epoch == 5 else move["action"]["deadline_us"]) + 20_000
        witness = None
        deadline = time.monotonic_ns() // 1000 + 2_000_000
        while time.monotonic_ns() // 1000 < deadline:
            try:
                s = next_sample()
            except RuntimeError as error:
                if str(error) != "stale native acquisition":
                    raise
                continue
            if s["source_us"] >= cutoff:
                if s["native"].get("move", {}).get("requested") != [0, 0, 0]:
                    raise RuntimeError("native loop did not independently discard expired task input")
                witness = s
                break
        if witness is None:
            raise RuntimeError("independent native expiry observation unavailable")
        expiry_witnesses.append(witness)
        transcript.append(status())
        move["sequence"] = 2
        rejection = task(dict(kind="move", descriptor=move))
        if rejection.get("accepted"):
            raise RuntimeError("native expiry allowed old action")
        transcript.append(rejection)
    return transcript, expiry_witnesses, expiry_moves, reference_trace


def build_simulator(root):
    """Construct the exact production model; also used by Stage 9A pin discovery."""
    sys.path.insert(0, str(Path(root) / "src"))
    from mjlab_microduck.sim import body_server as native
    import numpy as np
    world = native.World(native.DEFAULT_SCENE)
    body = native.Body(world, 0)
    body.place(None, native.HOME_TRUNK_Z, offset_y=0.0)
    world.bodies.append(body)
    native.mujoco.mj_forward(world.model, world.data)
    model_bytes = np.zeros(native.mujoco.mj_sizeModel(world.model), dtype=np.uint8)
    native.mujoco.mj_saveModel(world.model, buffer=model_bytes)
    digest = hashlib.sha256(model_bytes.tobytes()).hexdigest()
    return native, np, world, body, digest


def readiness_report(rpc, next_sample, identity, subscribed, assets, model_digest, engine):
    """Read-only post-provisioning checks. Never installs/admit/moves a task."""
    if any(subscribed.get(slot) != Path(path).name
           for slot, path in zip(("walk", "stand"), assets)):
        raise RuntimeError("both exact policies must be loaded and warmed by robotd")
    observations = collect_standing(next_sample, tuple(identity[k] for k in ("controller", "body", "world")))
    head = None
    for s in observations:
        source, sim, seq, tick = s["source_us"], s["simulation_us"], s["sequence"], s["native"].get("t_ns")
        if (type(tick) is not int or not source <= tick // 1000 < source + 20_000
                or source <= 0 or sim <= 0 or seq <= 0):
            raise RuntimeError("readiness observation/native acquisition stale")
        if head is not None:
            old_source, old_sim, old_seq, old_tick = head
            delta = source - old_source
            if (not 0 < delta < 200_000 or seq <= old_seq or tick <= old_tick
                    or sim <= old_sim or not delta // 2 <= sim - old_sim <= delta * 2 + 20_000):
                raise RuntimeError("readiness clocks paused/reset or observation gap")
        head = source, sim, seq, tick
    now = time.monotonic_ns() // 1000
    if not observations[-1]["source_us"] <= now < observations[-1]["source_us"] + 200_000:
        raise RuntimeError("readiness final observation stale")
    status = rpc("robot.task", dict(kind="status", protocol="microduck-task-v1"))
    if (status.get("identity") != identity or status.get("fenced") is not True
            or status.get("accepted") is not True or status.get("reason") != "not_installed"
            or status.get("installed") is not None or status.get("action") is not None
            or status.get("high_water_epoch") != 0 or status.get("sequence") != 0
            or status.get("consumed_sequence") != 0
            or status.get("protocol") != "microduck-task-v1"
            or status.get("profile") != "reference-velocity-v1"):
        raise RuntimeError("readiness native status/protocol mismatch")
    return dict(stage="9A", readiness="READY", qualification=False, release=False,
                modelSha256=model_digest, mujocoVersion=engine, identity=identity,
                policyAvailability={slot: subscribed[slot] for slot in ("walk", "stand")},
                nativeStatus=status, observations=observations,
                namespaces=[os.readlink("/proc/self/ns/" + n) for n in ("mnt", "pid", "net")])


def run(*, readiness_only=False):
    if len(sys.argv) not in (8, 12):
        raise RuntimeError("owned launcher arguments required")
    robotd, root, params, assets_json, daemon, body_id, world_id = sys.argv[1:8]
    pins = json.loads(sys.argv[8]) if len(sys.argv) == 12 else None
    identity = None
    original_params, original_assets = params, json.loads(assets_json)
    if pins:
        check_native_artifacts(pins, original_params, original_assets)
        params, assets_json = snapshot_native_artifacts(params, assets_json)
        snapshot_digest = artifact_snapshot_digest(params, json.loads(assets_json))
        identity = dict(environment=sys.argv[9], domain=sys.argv[10], body_ref=sys.argv[11],
                        body=body_id, world=world_id)
    # Private loopback namespace, one body connection, no camera/ToF gateways or
    # external clients. Native stepping code and real RemoteIo remain unchanged.
    native, np, world, body, model_digest = build_simulator(root)
    if pins and (model_digest != pins["compiledModelSha256"] or native.mujoco.__version__ != pins["mujocoVersion"]):
        raise RuntimeError("exact compiled simulator mismatch")
    records = collections.deque(maxlen=32)
    original = body.sensors
    sequence = 0
    records_lock = threading.Lock()

    def sensors():
        nonlocal sequence
        acquisition_us = time.monotonic_ns() // 1000
        measured = original()
        with world.lock:
            # No splicing a later root pose into an older sensor acquisition.
            if float(world.data.time) != measured["sim_time"]:
                return measured
            position = [float(x) for x in world.data.qpos[body.trunk:body.trunk + 3]]
            quat = world.data.qpos[body.trunk + 3:body.trunk + 7].copy()
            velocity = world.data.qvel[body.trunk_dof:body.trunk_dof + 6].copy()
            gravity = native.gravity_in_trunk(quat)
            yaw = math.atan2(2 * (quat[0] * quat[3] + quat[1] * quat[2]),
                             1 - 2 * (quat[2] ** 2 + quat[3] ** 2))
            oracle = dict(position=position, yaw=yaw,
                          linear_speed=float(np.linalg.norm(velocity[:3])),
                          angular_speed=float(np.linalg.norm(velocity[3:])),
                          uncertainty=0.000001,
                          upright=bool(gravity[2] < -0.9 and position[2] >= 0.08))
        with records_lock:
            sequence += 1
            records.append(dict(source_us=acquisition_us,
                                simulation_us=round(measured["sim_time"] * 1000000),
                                sequence=sequence, oracle=oracle))
        return measured

    body.sensors = sensors  # instrumentation only, returning original sensor data
    server = native.Server(("127.0.0.1", 7801), native.Handler)
    server.body = body
    threading.Thread(target=server.serve_forever, daemon=True).start()
    threading.Thread(target=native.run, args=(world, True), daemon=True).start()
    private = Path("/tmp/pastey-microduck-gate-a")
    private.mkdir(mode=0o700)
    endpoint = private / "robotd.sock"
    command = [robotd, "--sim", "127.0.0.1:7801", "--socket", str(endpoint), "--params", params]
    if pins:
        identity_path = private / "identity.json"
        identity_path.write_text(json.dumps(identity))
        command += ["--pastey-task-identity", str(identity_path)]
    process = subprocess.Popen(command,
                               stdin=subprocess.DEVNULL, stdout=sys.stderr, stderr=sys.stderr)
    shutting_down = threading.Event()
    def watch_native():
        process.wait()
        if not shutting_down.is_set():
            os._exit(1)  # private child pipe closes; never transparently restart a daemon
    threading.Thread(target=watch_native, daemon=True).start()
    try:
        stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stream.settimeout(2)
        deadline = time.monotonic() + 10
        while not endpoint.exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("supervised robotd did not start")
            time.sleep(0.01)  # readiness only; never authority or measurement timing
        stream.connect(str(endpoint))
        endpoint.unlink()  # existing sole connection survives; competing opens fail
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            probe.connect(str(endpoint))
            raise RuntimeError("competing writer remains reachable")
        except FileNotFoundError:
            pass
        finally:
            probe.close()
        wire = stream.makefile("rwb", buffering=0)
        native_id = 0
        latest = None
        minimum_source_us = 0

        def sample(state):
            nonlocal latest
            ns = state.get("t_ns")
            latest = None  # Never return a cached observation after a mapping failure.
            if not isinstance(ns, int) or ns <= 0:
                return
            with records_lock:
                acquired = next((r.copy() for r in reversed(records)
                                 if r["source_us"] * 1000 <= ns), None)
            if acquired is None or time.monotonic_ns() // 1000 - acquired["source_us"] >= 200000:
                return
            # Preserve native commands/odometry as diagnostics; they are never
            # substituted for modeled root velocity or qualified uncertainty.
            acquired.update(daemon=daemon, body=body_id, world=world_id,
                            native={key: state.get(key) for key in
                                    ("t", "t_ns", "move", "odom", "policy", "safety")})
            latest = acquired

        def rpc(method, args):
            nonlocal native_id
            native_id += 1
            wire.write((json.dumps(dict(jsonrpc="2.0", id=native_id,
                                        method=method, params=args)) + "\n").encode())
            rpc_deadline = time.monotonic() + 2
            while True:
                if time.monotonic() >= rpc_deadline:
                    raise RuntimeError("native RPC timeout")
                raw = wire.readline(MAX_LINE + 1)
                if not raw or len(raw) > MAX_LINE:
                    raise RuntimeError("native stream lost/oversized")
                message = json.loads(raw)
                if message.get("method") == "robot.state":
                    sample(message["params"])
                elif message.get("id") == native_id:
                    if "error" in message:
                        return dict(accepted=False)
                    return message["result"]

        if pins:
            status = rpc("robot.task", dict(kind="status", protocol="microduck-task-v1"))
            actual = status.get("identity", {})
            if (status.get("protocol") != "microduck-task-v1" or status.get("profile") != "reference-velocity-v1"
                    or status.get("fenced") is not True
                    or any(actual.get(k) != v for k, v in identity.items())
                    or not actual.get("controller")):
                raise RuntimeError("native task identity/protocol/mode unavailable")
            identity = actual
            daemon = actual["controller"]
        subscribed = rpc("robot.subscribe", dict(hz=50))
        assets=[Path(p) for p in json.loads(assets_json)]
        if not assets or len({p.name for p in assets})!=len(assets):
            raise RuntimeError("ambiguous policy artifact manifest")
        for slot in ("walk", "stand"):
            name=subscribed.get(slot)
            if name is not None and name not in {p.name for p in assets}:
                raise RuntimeError("unmanifested native policy")
        if subscribed.get("unavailable") is not None:
            raise RuntimeError("native policy unavailable; no Gate A qualification")
        if subscribed.get("accepted") is not True:
            raise RuntimeError("subscription refused")
        def next_sample():
            # Consume queued subscription frames without an authority-changing
            # RPC. Idle monitoring is 20 Hz while robot.state is 50 Hz; returning
            # one FIFO frame would eventually turn a healthy run into stale data.
            until = time.monotonic() + 2
            while time.monotonic() < until:
                raw_state = wire.readline(MAX_LINE + 1)
                if not raw_state or len(raw_state) > MAX_LINE:
                    raise RuntimeError("state source lost")
                message = json.loads(raw_state)
                if message.get("method") != "robot.state":
                    raise RuntimeError("unexpected native frame")
                sample(message["params"])
                if select.select([stream], [], [], 0)[0]:
                    continue
                if latest is not None and latest["source_us"] >= minimum_source_us:
                    return latest
                if latest is None:
                    raise RuntimeError("stale native acquisition")
            raise RuntimeError("fresh native acquisition timeout")

        # Drain any pre-ACK frames inside rpc; proof begins after that ACK.
        provision(rpc, next_sample, (daemon, body_id, world_id))
        if readiness_only:
            if not pins:
                raise RuntimeError("Stage 9A requires exact native artifacts")
            report = readiness_report(rpc, next_sample, identity, subscribed,
                                      json.loads(assets_json), model_digest, native.mujoco.__version__)
            check_native_artifacts(pins, original_params, original_assets)
            if artifact_snapshot_digest(params, json.loads(assets_json)) != snapshot_digest:
                raise RuntimeError("readiness artifact snapshot changed")
            emit(report)  # Deliberately not a production Hello or qualification bundle.
            return
        bundle = None
        if pins:
            transcript, expiry_witnesses, expiry_moves, reference_trace = native_probes(rpc, next_sample, process, identity)
            # The last probe leaves authority closed. No probe renews a Core task.
            # Establish fresh measured rest again before sealing enrollment.
            observations = collect_standing(next_sample, (daemon, body_id, world_id))
            bundle = dict(producer="pastey.microduck.qualification.v1", pins=pins,
                artifactDigest=hashlib.sha256(Path(robotd).read_bytes()).hexdigest(),
                controller=daemon, body=body_id, world=world_id, modelSha256=model_digest,
                engine=native.mujoco.__version__, namespaces=[os.readlink("/proc/self/ns/"+n) for n in ("mnt","pid","net")],
                parentNamespaces=json.loads(os.environ["PASTEY_PARENT_NAMESPACES"]),
                soleWriter=True, realSimulation=True, nativePauseExpiry=True,
                resetPolicy="owned-world-no-reset-api-replacement-launch-only",
                mechanism=transcript, observations=observations, expiryWitnesses=expiry_witnesses,
                expiryMoves=expiry_moves, referenceTrace=reference_trace)
            if model_digest != pins["compiledModelSha256"] or native.mujoco.__version__ != pins["mujocoVersion"]:
                raise RuntimeError("exact compiled simulator mismatch")
        emit(dict(version=1, provisioned=True, clock_us=time.monotonic_ns() // 1000, single_writer=True,
                  model_digest=model_digest, simulation_engine=native.mujoco.__version__,
                  simulation=True, namespaces=[os.readlink("/proc/self/ns/" + n)
                                                for n in ("mnt", "pid", "net")],
                  daemon=daemon, body=body_id, world=world_id, gate_b=bundle))
        last_sequence = 0
        for raw in sys.stdin:
            if len(raw) > MAX_LINE or process.poll() is not None:
                raise RuntimeError("supervision lost")
            request = json.loads(raw)
            if request == {"operation": "clock"}:
                minimum_source_us = time.monotonic_ns() // 1000
                emit(dict(clock_us=minimum_source_us))
                continue
            if set(request) != {"sequence", "request"}:
                raise RuntimeError("uncorrelated Gate A request")
            request_sequence = request["sequence"]
            if type(request_sequence) is not int or request_sequence <= last_sequence:
                raise RuntimeError("replayed Gate A request")
            last_sequence = request_sequence
            request = request["request"]
            operation = request.get("operation")
            if pins:
                check_native_artifacts(pins, original_params, original_assets)
                if snapshot_digest != artifact_snapshot_digest(params, json.loads(assets_json)):
                    raise RuntimeError("owned artifact snapshot changed")
            if operation == "task" and pins and set(request) == {"operation", "request"}:
                result = rpc("robot.task", request["request"])
                if result.get("identity") != identity:
                    raise RuntimeError("native controller/body/world changed")
                emit(dict(sequence=request_sequence, reply=dict(accepted=None, sample=None, native=result)))
            elif operation == "move" and not pins and set(request) == {"operation", "vx", "vy", "vyaw"}:
                if (request["vx"], request["vy"], request["vyaw"]) != (0.05, 0, 0):
                    raise RuntimeError("non-reference velocity")
                result = rpc("robot.move", dict(vx=0.05, vy=0, vyaw=0))
                emit(dict(sequence=request_sequence, reply=dict(accepted=result.get("accepted"), sample=None)))
            elif operation == "stop" and not pins and set(request) == {"operation"}:
                result = rpc("robot.stop", {})
                emit(dict(sequence=request_sequence, reply=dict(accepted=result.get("accepted"), sample=None)))
            elif operation == "sample" and set(request) == {"operation"}:
                emit(dict(sequence=request_sequence, reply=dict(accepted=None, sample=next_sample())))
            else:
                raise RuntimeError("unsupported Gate A operation")
    finally:
        shutting_down.set()
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        server.shutdown()


if __name__ == "__main__":
    with contextlib.redirect_stdout(sys.stderr):
        run()
