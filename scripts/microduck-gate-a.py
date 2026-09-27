#!/usr/bin/env python3
"""Owned Linux Gate A supervisor. No hardware or native fence.

Only the Rust launcher starts this inside fresh bwrap mount/PID/network namespaces.
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
import subprocess
import sys
import threading
import time

OUTPUT = sys.stdout
MAX_LINE = 65536


def emit(value):
    OUTPUT.write(json.dumps(value, allow_nan=False, separators=(",", ":")) + "\n")
    OUTPUT.flush()


def run():
    robotd, root, params, assets_json, daemon, body_id, world_id = sys.argv[1:]
    sys.path.insert(0, str(Path(root) / "src"))
    from mjlab_microduck.sim import body_server as native
    import numpy as np

    # Private loopback namespace, one body connection, no camera/ToF gateways or
    # external clients. Native stepping code and real RemoteIo remain unchanged.
    world = native.World(native.DEFAULT_SCENE)
    body = native.Body(world, 0)
    body.place(None, native.HOME_TRUNK_Z, offset_y=0.0)
    world.bodies.append(body)
    native.mujoco.mj_forward(world.model, world.data)
    model_bytes = np.zeros(native.mujoco.mj_sizeModel(world.model), dtype=np.uint8)
    native.mujoco.mj_saveModel(world.model, buffer=model_bytes)
    model_digest = hashlib.sha256(model_bytes.tobytes()).hexdigest()
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
    process = subprocess.Popen([robotd, "--sim", "127.0.0.1:7801", "--socket", str(endpoint), "--params", params],
                               stdin=subprocess.DEVNULL, stdout=sys.stderr, stderr=sys.stderr)
    def watch_native():
        process.wait()
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

        def sample(state):
            nonlocal latest
            ns = state.get("t_ns")
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
            while True:
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
        emit(dict(version=1, clock_us=time.monotonic_ns() // 1000, single_writer=True,
                  model_digest=model_digest, simulation_engine=native.mujoco.__version__,
                  simulation=True, namespaces=[os.readlink("/proc/self/ns/" + n)
                                                for n in ("mnt", "pid", "net")],
                  daemon=daemon, body=body_id, world=world_id))
        last_sequence = 0
        for raw in sys.stdin:
            if len(raw) > MAX_LINE or process.poll() is not None:
                raise RuntimeError("supervision lost")
            request = json.loads(raw)
            if request == {"operation": "clock"}:
                emit(dict(clock_us=time.monotonic_ns() // 1000))
                continue
            if set(request) != {"sequence", "request"}:
                raise RuntimeError("uncorrelated Gate A request")
            request_sequence = request["sequence"]
            if type(request_sequence) is not int or request_sequence <= last_sequence:
                raise RuntimeError("replayed Gate A request")
            last_sequence = request_sequence
            request = request["request"]
            operation = request.get("operation")
            if operation == "move" and set(request) == {"operation", "vx", "vy", "vyaw"}:
                if (request["vx"], request["vy"], request["vyaw"]) != (0.05, 0, 0):
                    raise RuntimeError("non-reference velocity")
                result = rpc("robot.move", dict(vx=0.05, vy=0, vyaw=0))
                emit(dict(sequence=request_sequence, reply=dict(accepted=result.get("accepted"), sample=None)))
            elif operation == "stop" and set(request) == {"operation"}:
                result = rpc("robot.stop", {})
                emit(dict(sequence=request_sequence, reply=dict(accepted=result.get("accepted"), sample=None)))
            elif operation == "sample" and set(request) == {"operation"}:
                # Wait for a new notification, not a cached receipt-time refresh.
                raw_state = wire.readline(MAX_LINE + 1)
                if not raw_state or len(raw_state) > MAX_LINE:
                    raise RuntimeError("state source lost")
                message = json.loads(raw_state)
                if message.get("method") == "robot.state":
                    sample(message["params"])
                else:
                    raise RuntimeError("unexpected native frame")
                emit(dict(sequence=request_sequence, reply=dict(accepted=None, sample=latest)))
            else:
                raise RuntimeError("unsupported Gate A operation")
    finally:
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
