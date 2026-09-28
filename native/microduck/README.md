# Native MicroDuck Gate B: Stage 8 mechanism and Stage 9 release gate

This is a reproducible source overlay for MicroDuck commit
`a9ec4b2079ef8ee7904014089c885bb07d57d63c`. It is not an upstream release,
a qualification record or a bundled production binary. The authoritative
Pastey contract is [physical implementation](../../docs/physical-environment-implementation.md#stage-8-native-consumption-contract).

Apply to a clean checkout at that exact commit:

```bash
python3 -B scripts/apply-microduck-gate-b.py /absolute/path/to/microduck
cargo test --manifest-path /absolute/path/to/microduck/Cargo.toml -p robotd -- --test-threads=1
cargo build --manifest-path /absolute/path/to/microduck/Cargo.toml -p robotd
PASTEY_GATE_B_ROBOTD=/absolute/path/to/microduck/target/debug/robotd python3 -B scripts/test-microduck-gate-b.py
PASTEY_GATE_B_ROBOTD=/absolute/path/to/microduck/target/debug/robotd cargo test --manifest-path src-tauri/Cargo.toml real_robotd_ -- --ignored --test-threads=1
```

The apply script checks the pin, clean tree and patch applicability before changing
source. It copies only the two overlay files. It does not fetch dependencies,
launch a daemon, write qualification records or promote a profile. Tests that bind
Unix sockets require local IPC access. The integration commands launch real
`robotd --fake --no-policy`; FakeIo replaces actuator I/O. They exercise real IPC,
native clock, connection ownership, control loop and the Pastey local/remote lane.
They do not exercise PPO inference or MuJoCo physics.

## Upstream files changed

| File | Reason |
|---|---|
| `duck-ipc-proto/src/lib.rs` | Export one versioned task metadata module; leave the existing global method enum unchanged. |
| `duck-ipc-proto/src/task_authority.rs` (overlay) | Install/admit/move/fence/status descriptors and native receipts, with strict deserialization. |
| `robotd/src/intents.rs` | Own one native task guard beside existing intents. |
| `robotd/src/task_authority.rs` (overlay) | Native epoch/session/action validation, fixed native deadlines, connection loss, fresh boot identity and deterministic guard tests. |
| `robotd/src/main.rs` | Optional isolated task-mode launch, IPC dispatch, unowned mutation rejection, native I/O-loss invalidation and the narrow consumption/apply guard, with tests inside the real loop. |

No PPO/controller intelligence, Safety algorithm, RobotIo implementation, RemoteIo,
MuJoCo physics, model tensor contract or unrelated daemon method table is changed.
Default launch leaves legacy Gate A behavior intact. Task mode rejects hardware
launches and unowned mutating methods/notifications. Its sole provisioning exception
is exact `robot.enable {on:true,toggle:false}` before the first native installation.
It is outside task authority and requires the trusted isolated launcher.

## Native boundary and limits

`robot.task` receives metadata. Move ACK means queued admission, not effect. The
control loop takes the same guard mutex used by install/fence, validates and
consumes tagged pending intent before controller shaping, and retains the guard
through `Safety::apply`. Contention produces zero task twist and discards the tick's
motion targets rather than blocking the control loop. Immediately before apply,
the loop resamples native time. Expiry during the frame discards its computed
motion targets and holds the last known pose for that frame. Subsequent ticks use
zero twist through the existing controller. A fence ACK cannot precede application
of an earlier frame holding that guard. Frames already applied are not undone.
No receipt proves physical rest, torque-off or emergency stop.

Native `CLOCK_MONOTONIC` bounds are fixed at install/admit: lease <=3 seconds,
action <=1 second and <=lease, missing valid refresh >=200 ms closes the task.
Expiry is checked by the loop without Pastey traffic. Pastey maps remaining Core
lifetime after a <=20 ms status round trip onto the earlier native sample, so
transport delay shortens validity. No retry or refresh changes a deadline. This
assumes progressing same-Host monotonic clocks. Stage 9 binds awake/progressing-loop
conditions, tests expiry on SIGSTOP resume, and excludes Host suspend or continuous
protection while frozen. There is no independent watchdog. Real simulator evidence
for those conditions remains pending.

Exact descriptor duplicates are idempotent; higher epochs require a new install.
A foreign connection cannot take an active owner. A stale fence cannot affect a
newer installation. Invalid current-owner move closes its task window; foreign
rejections cannot revoke another connection's ownership. Connection EOF/error
clears pending intent. A lost/partial ACK is uncertainty; it is never recovered by
resuming the old session. The adapter does not reconnect.

Every native boot generates a fresh OS-random controller incarnation. No task
state is restored. Old installation and buffered command descriptors mismatch the
new identity even at the same socket name. Native read/controller/write failure
publishes a sticky atomic launch-loss latch even when the task mutex is contended.
Consumption and IPC fold that latch into invalidated state; no same-launch install
or higher epoch can clear it. Failure handling never waits for the task mutex.
Only a fresh launch/controller incarnation can recover.
Body/world identity comes from the trusted isolated launch, not task metadata. An
in-place simulator/body reset without a native I/O discontinuity is unsupported;
qualification must prove that reset cannot preserve this launch binding. The
internal Pastey constructor requires a sealed live binding and a private owned
socket; arbitrary JSON, paths, status replies or SQLite rows cannot mint it.
This mechanism does not authenticate arbitrary public clients or implement operator
preemption. Stage 9 adds a separate exact owned simulator qualification producer,
immutable evidence record and explicit release/withdrawal gate. It never promotes
Gate A automatically; the production manifest remains `PENDING_ENVIRONMENT`.

The deterministic harness injects pauses before real consumption and immediately
before real apply, and holds the task mutex while injecting an inference-result
error or an actuator write failure through the real loop. Guard tests cover fence/refresh serialization, partial requests,
replay, controller replacement and native deadlines. The process harness delays
ACK reads, restarts robotd/controller, drops a partial-fence connection and waits
for independent native expiry. Process restart replaces the controller incarnation;
there is no supported separate in-place controller restart that restores task state.

## Stage 9 production qualification

The [exact qualification contract](../../docs/physical-environment-implementation.md#stage-9-exact-simulator-qualification-and-release) defines the trusted Host installation-owner path, owned source rebuild, independent MuJoCo observations, one-second reference trace, expiry probes, reset/suspend boundary and record/release semantics. [profile-v1.json](profile-v1.json) pins both source revisions and explicitly leaves unknown real model/policy/Python/ORT identities null. Resource locators cannot override these compiled pins or mint qualification.

The production manifest remains pending. Stage 9A located and byte-verified the exact official walk/stand artifacts, but this macOS host cannot establish the Linux simulator/controller environment. No real simulator/controller qualification or production release record exists. Rust/native-oracle and Python orchestration tests exercise mechanism/qualification plumbing only; they cannot fill the missing golden identities or authorize release. No hardware qualification is performed.

Pinned robotd stamps `RobotState.t_ns` immediately before synchronous `Safety::read()` / `RemoteIo::read()`, retaining the last successful read-start stamp while coasting. The simulator acquisition normally follows it. The [acquisition clock contract](../../docs/physical-environment-implementation.md#stage-6-implementation-contract-and-qualification-limits) documents first-read correlation, strict sub-20-ms skew, the unchanged 200 ms freshness ceiling, advancing clocks and post-enable/expiry cutoffs; reversed microsecond ordering rejects. Simulator progress uses the same documented one-second, 272 ms cumulative phase envelope across Python and Rust, with strictly advancing source/native/sequence, non-regressing simulation time and source gaps below 200 ms. Fresh adjacent reads may share the current world time, but a sustained pause exhausts the unchanged phase envelope. The pinned 20 ms batch/deadline loop provides best-effort pacing, not an adjacent-pair ratio guarantee. Provisioning/readiness requires a complete window. Failure diagnostics include bounded previous/current clocks and deltas without identity, permit or path data.

## Stage 9A reproducible environment and readiness

[environment-v1.json](environment-v1.json) records independently retrieved upstream artifact provenance. It is **not** a production profile, a partial pin promotion or qualification evidence. The pinned native source's `Cargo.toml`/`scripts/seed-policies.sh` require the official policy set v5. That tag resolved to immutable [policy revision 1b56c396](https://huggingface.co/pollen-robotics/microduck-policies/tree/1b56c396825c052a4e26e95cf2b8d8298af9e9b4). Its manifest and both downloaded policies were independently hashed against the immutable upstream metadata. The reference pair follows the pinned native `scripts/duck-sim` launcher:

| Resource | Exact source / identity |
|---|---|
| Walk | `alpha_walking.onnx`, SHA-256 `e36332d383997d51401897734cd3e79cf5038406feddb18b4d57ecfb141daa6c` |
| Stand | `alpha_stand.onnx`, SHA-256 `1569268713e40deea795dd2922dba50d3621e15a872855408b6b1b125b1c094b` |
| Parameters | Pinned native `deploy/robotd.toml`, template SHA-256 `1b9ddb2010405812df195d825c5765f3d2ace7360e7c7846af43ffe507fac742`; generated exact reference artifact adds only walk/stand locators and explicit `none` for the five unsupported slots |
| Model | Pinned RL `src/mjlab_microduck/robot/microduck/scene.xml`, constructed through upstream `World`/`Body`, one duck, HOME placement, native timestep; compiled `mj_saveModel` bytes must be measured on Linux |
| Python / MuJoCo / ORT | Private Python 3.12 venv; pinned RL `uv.lock` (SHA-256 `2eeeb680025baa737e7ccf3a87da3695a9d49b228de0bb60d8af0c022ab5f1aa`) selects MuJoCo 3.10.0, NumPy 2.4.1 and ONNX Runtime 1.24.4; robotd consumes the exact Linux `onnxruntime/capi/libonnxruntime.so.*` from that wheel via `ORT_DYLIB_PATH` |

The newer default `velstand.onnx` is a separate single-policy configuration. It does not replace the required walk/stand pair. No policy training/export is needed: no checkpoint, training package or similarly named artifact is substituted. Unknown model/environment/executable/native-ORT production hashes stay null.

Use a Linux x86_64 or aarch64 host with glibc compatible with the locked wheels (ORT requires >=2.28), Python 3.12 with venv/pip, Git, a C compiler/linker, Rust >=1.89/Cargo and bubblewrap. Its security policy must permit private user/mount/PID/network namespaces. The locked MuJoCo/GLFW import must have its host shared-library dependencies available; failures are reported rather than replaced by a different simulator. No GPU, training stack, container or VM is required by this headless body-server path. The following Linux sequence is prepared but **has not been executed on Linux here**:

```bash
# Keep source checkouts clean; the script archives them into its new work directory.
git clone --no-checkout https://github.com/pollen-robotics/microduck.git /absolute/sources/microduck
git -C /absolute/sources/microduck checkout --detach a9ec4b2079ef8ee7904014089c885bb07d57d63c
git clone --no-checkout https://github.com/pollen-robotics/microduck_rl.git /absolute/sources/microduck_rl
git -C /absolute/sources/microduck_rl checkout --detach cb70b792312d559a4da09064d92009079671815f

/absolute/bin/python3.12 -B scripts/prepare-microduck-gate-b-environment.py \
  --native-source /absolute/sources/microduck \
  --rl-source /absolute/sources/microduck_rl \
  --python /absolute/bin/python3.12 \
  --work /absolute/new-external-stage9a-directory \
  --report /absolute/new-stage9a-report.json
```

Both output paths must be new and outside Pastey. This command fetches only the immutable manifest/pair and the wheel dependency closure of MuJoCo, NumPy and ORT from the pinned lock, with pip `--require-hashes --only-binary=:all: --no-deps`. All 16 runtime packages (including `etils[epath]` dependencies) come from that lock; Torch/CUDA, mjlab training and BAM Python are not imported by upstream's headless body server. Unexpected lock sources, markers or missing exact wheels fail instead of resolving alternatives. The upstream lock's unrelated bcrypt wheel under ORT is excluded by distribution/version checking. Venv bootstrap pip is supplied by the selected Python's `ensurepip` and is included in the resulting environment digest.

The script uses the existing owned clean archive/overlay builder with `cargo build --locked --release -p robotd`. It runs fresh bubblewrap namespaces with the production mount/PID/network isolation pattern. The first checks exact imports and constructs the single body/model for candidate pin discovery; a separate source copy at another path must reproduce the same compiled model identity, since Core packages each future launch at a fresh path. The readiness namespace independently constructs that model again, verifies the candidate bytes, starts real `robotd --sim`, unlinks its sole socket, verifies the native task protocol/profile/incarnation, subscribes, provisions enable and acquires advancing, fresh, measured upright observations. Both policies must be present; upstream's native loader validates and warms each network. A final status must still be `not_installed`, with no installed/action/sequence/epoch state. It performs **no task install/admit/move, reference trace, expiry qualification experiment, Core enrollment, reviewed action, consequence, L7 or release**.

The readiness output has `stage: "9A"`, `qualification: false`, `release: false`; it is deliberately not the production supervisor Hello/evidence bundle and cannot be used for enrollment. The script writes candidate `ProfilePinsV1` only after all real checks pass, including different namespace IDs and unchanged pre/post runtime artifact hashes. It never edits [profile-v1.json](profile-v1.json). Independently review the JSON's Linux/tool/interpreter/import versions, consumed paths, overlay/binary digests, reproduced model identities, measured observations and native receipt before copying all seven concrete pin fields (eight resource identities) into the compiled profile. The parameters digest hashes the original reference file; the owned supervisor only rewrites the two policy locators in private copies and checks their bytes throughout readiness.

Hashing matches production: SHA-256 over raw file bytes; the Python executable follows its canonical symlink target. Venv SHA-256 covers compact UTF-8 JSON of a sorted map of relative POSIX paths: files map to content hashes, and directory aliases map to `["directorySymlink", literal link text, canonical root-relative target]`. Relative directory symlinks such as `lib64 -> lib` are accepted only when their canonical targets remain inside the same canonical venv root. Absolute directory links, escapes, broken links and cyclic directory graphs (including ancestor/sibling cycles) are rejected. The physical target tree is hashed once, not traversed again through aliases; changing/removing an alias changes its identity. `__pycache__` and `.pyc` file contents remain excluded, but directory aliases there are still validated and identified. File symlinks retain target-byte hashing, including the standard external Python executable link. [Shared golden fixtures](../../scripts/fixtures/microduck-environment-digest-v1.json) are independently checked in Python and by Rust's production hashing function. Environments without directory aliases retain their existing digest. Environment pins identify the actual installation, including its paths/bootstrap files; another host must establish/review its own exact installation rather than copy unrelated hashes.

Current Stage 9A result: **BLOCKED for Stage 9B**. Earlier local validation verified clean exact native/RL sources, template/lock bytes, official manifest/policy bytes and clean overlay reproduction, including a macOS release build. Reported Ubuntu ARM64 run `006` now passes the full provisioning acquisition/timing layer, including a completed cumulative progress window, and reaches torque on, the completed two-second HOME_RAMP, policy ownership and native `fallen=false`. It times out before proving 200 ms continuous settling; the failing settling predicate is not yet established. The timeout now reports a bounded structured summary of settling failures, streak duration and final/extreme measurements, including supervisor-local gravity/height components. Native not-fallen remains distinct from Pastey's stricter upright predicate. This diagnostic change is locally regression-tested; the real Ubuntu run has not been repeated here. Acceptance thresholds/deadlines, evidence schema and production profile are unchanged. No complete candidate/production pin set or qualification/release record was committed. Run the command above on the supported Linux host and independently review its real report; final qualification/release remains a separate Stage 9B task.
