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

The sole native movement is `REFERENCE_TWIST`, defined by `REFERENCE_FORWARD_MPS = 0.08 m/s` in the [task protocol](overlay/duck-ipc-proto/src/task_authority.rs). This bounded payload is retained during the single-policy migration; it is no longer derived from a standing threshold or EMA crossing. Guard, adapter, owned Python producer and native fixtures share the protocol constant. Measured displacement must still pass 0.01–0.1 m and the unchanged lateral, upright, freshness and settling gates. Native authority limits and exact payload rejection are unchanged.

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
Gate A automatically; reviewed artifact pins permit qualification attempts only.

The deterministic harness injects pauses before real consumption and immediately
before real apply, and holds the task mutex while injecting an inference-result
error or an actuator write failure through the real loop. Guard tests cover fence/refresh serialization, partial requests,
replay, controller replacement and native deadlines. The process harness delays
ACK reads, restarts robotd/controller, drops a partial-fence connection and waits
for independent native expiry. Process restart replaces the controller incarnation;
there is no supported separate in-place controller restart that restores task state.

## Stage 9 production qualification

The [exact qualification contract](../../docs/physical-environment-implementation.md#stage-9-exact-simulator-qualification-and-release) defines the trusted Host installation-owner path, owned source rebuild, independent MuJoCo observations, one-second reference trace, expiry probes, reset/suspend boundary and record/release semantics. [profile-v1.json](profile-v1.json) pins both source revisions and the reviewed run-008 model/Python/ORT identities and the newly verified parameter/velstand bytes. Resource locators cannot override these compiled pins or mint qualification. The reference producer schedules same-action refreshes from the fixed native deadline, with 50 ms cadence/service margin, a final accepted receipt at or beyond deadline minus 150 ms, and a strict 50 ms expiry guard. It continues independent sampling through expiry, then uses dedicated reference settling collection to retain correlated upright/not-fallen deceleration frames continuously through a real >=500 ms final rest interval. Every raw acquisition and retained source/native/sequence progression is checked; approximately 10 Hz thinning retains settling starts/interruptions and real bridge frames for strict <200 ms gaps. The complete trace is capped at 64 samples and fails closed on acquisition loss or exhaustion. Pure standing collection still discards non-standing prefixes. Reported run `009i` exposed the former standing-suffix splice after deceleration; this producer correction preserves all continuity, settling and qualification predicates. Reported run `009f` proved the former nineteen-sleep schedule could exhaust its budget; this correction changes neither native limits nor qualification/release predicates. Rejected refreshes report the returned receipt without a diagnostic status RPC.

The compiled manifest is `PENDING_ENVIRONMENT` until a fresh Linux readiness report is independently reviewed. Run `008` reviewed the former alpha pair; it does not establish readiness or qualification for velstand. The source-supported migration replaces only parameter/policy pins and retains the exact model/Python/ORT identities. No new simulator or hardware run was performed; `qualification=false` and `release=false`. Local regressions cannot establish displacement or settling.

Pinned robotd stamps `RobotState.t_ns` immediately before synchronous `Safety::read()` / `RemoteIo::read()`, retaining the last successful read-start stamp while coasting. The simulator acquisition normally follows it. The [acquisition clock contract](../../docs/physical-environment-implementation.md#stage-6-implementation-contract-and-qualification-limits) documents first-read correlation, strict sub-20-ms skew, the unchanged 200 ms freshness ceiling, advancing clocks and post-enable/expiry cutoffs; reversed microsecond ordering rejects. Simulator progress uses the same documented one-second, 272 ms cumulative phase envelope across Python and Rust, with strictly advancing source/native/sequence, non-regressing simulation time and source gaps below 200 ms. Fresh adjacent reads may share the current world time, but a sustained pause exhausts the unchanged phase envelope. The pinned 20 ms batch/deadline loop provides best-effort pacing, not an adjacent-pair ratio guarantee. Provisioning/readiness requires a complete window. Failure diagnostics include bounded previous/current clocks and deltas without identity, permit or path data.

## Stage 9A reproducible environment and readiness

[environment-v1.json](environment-v1.json) records independently retrieved upstream artifact provenance. It is **not** a production profile, a partial pin promotion or qualification evidence. The pinned native source's `Cargo.toml`/`scripts/seed-policies.sh` require the official policy set v5. That tag resolved to immutable [policy revision 1b56c396](https://huggingface.co/pollen-robotics/microduck-policies/tree/1b56c396825c052a4e26e95cf2b8d8298af9e9b4). Its manifest SHA-256 remains `622048c2c23ea58942023f66fd16b189a875fd169e88d85beb16ebbe63b20c94`. The immutable official manifest declares `velstand.onnx` the default walk gait that stands at zero, with a standing entry pose. Downloaded velstand bytes independently match the repository's LFS SHA-256 `1c659be55da94bc5753b707de5c6a3e7c49931e05ca3b6991615cef1a8ba9a45`.

The native configuration is now `walk = "velstand.onnx"`, `stand = "none"`; other skill/posture slots remain explicitly disabled. At the already pinned native revision:

- [`PolicyParams::resolved_with`](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd-params/src/lib.rs#L1578) selects velstand with no stand network by default, and resolves the `none` sentinel to no network.
- [`Policy::will_stand`](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-control/src/policy.rs#L310) requires a loaded stand network. Without one, [`control.rs`](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd/src/control.rs#L525) selects Walk for both zero and forward twist, with native walk tuning. The 0.05 threshold does not select a network or standing tuning on this path.
- The unchanged task overlay clears expired/fenced task input and twist smoothing; subsequent ticks supply zero through the native controller. This lets velstand balance/settle without Pastey policy switching. A fence is still command exclusion, never evidence of rest.

The pinned `scripts/duck-sim` launcher explicitly chooses the older alpha pair; that example was the origin of Pastey's pair requirement, not a native capability constraint. The [official immutable manifest](https://huggingface.co/pollen-robotics/microduck-policies/blob/1b56c396825c052a4e26e95cf2b8d8298af9e9b4/manifest.json) and native defaults support the replacement. Real forward displacement and zero/fence settling within our bounds remain untested for this configuration.

Preparation derives reference parameter bytes from the unchanged SHA-pinned `deploy/robotd.toml`, inserting only the explicit slots. The walk locator is relative to the parameter file, so the new parameter SHA-256 is independent of the preparation directory: `10b729f4e6cf9bafda5557e71a000187a60b80a1a22627c7ac49ae877053a9bb`. The supervisor resolves and matches that locator to the one exact supplied artifact before copying it into its private namespace; only the walk locator is rewritten. Stand stays `none`. Native subscription must report the loaded walk file and no stand file (upstream omits absent optional fields). No policy training/export or controller changes are needed.

Use a Linux x86_64 or aarch64 host with glibc compatible with the locked wheels (ORT requires >=2.28), Python 3.12 with venv/pip, Git, a C compiler/linker, Rust >=1.89/Cargo and bubblewrap. Its security policy must permit private user/mount/PID/network namespaces. The locked MuJoCo/GLFW import must have its host shared-library dependencies available; failures are reported rather than replaced by a different simulator. No GPU, training stack, container or VM is required by this headless body-server path. The following readiness sequence was independently reviewed in real Ubuntu ARM64 run `008`; it was not rerun on this macOS host:

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

Both output paths must be new and outside Pastey. This command fetches only the immutable manifest/velstand artifact and the wheel dependency closure of MuJoCo, NumPy and ORT from the pinned lock, with pip `--require-hashes --only-binary=:all: --no-deps`. All 16 runtime packages (including `etils[epath]` dependencies) come from that lock; Torch/CUDA, mjlab training and BAM Python are not imported by upstream's headless body server. Unexpected lock sources, markers or missing exact wheels fail instead of resolving alternatives. The upstream lock's unrelated bcrypt wheel under ORT is excluded by distribution/version checking. Venv bootstrap pip is supplied by the selected Python's `ensurepip` and is included in the resulting environment digest.

The script uses the existing owned clean archive/overlay builder with `cargo build --locked --release -p robotd --target-dir <owned-source>/target`. The target path is absolute and explicitly chosen by preparation, so ambient `CARGO_TARGET_DIR` from an outer Pastey build cannot redirect the native artifact. The launcher still consumes only `<owned-source>/target/release/robotd`; it never falls back to a host target artifact. It runs fresh bubblewrap namespaces with the production mount/PID/network isolation pattern. The first checks exact imports and constructs the single body/model for candidate pin discovery; a separate source copy at another path must reproduce the same compiled model identity, since Core packages each future launch at a fresh path. The readiness namespace independently constructs that model again, verifies the candidate bytes, starts real `robotd --sim`, unlinks its sole socket, verifies the native task protocol/profile/incarnation, subscribes, provisions enable and acquires advancing, fresh, measured upright observations. The single velstand policy must be present; upstream's native loader validates and warms it. A final status must still be `not_installed`, with no installed/action/sequence/epoch state. It performs **no task install/admit/move, reference trace, expiry qualification experiment, Core enrollment, reviewed action, consequence, L7 or release**.

The readiness output has `stage: "9A"`, `qualification: false`, `release: false`; it is deliberately not the production supervisor Hello/evidence bundle and cannot be used for enrollment. The script writes candidate `ProfilePinsV1` only after all real checks pass, including different namespace IDs and unchanged pre/post runtime artifact hashes. It never edits [profile-v1.json](profile-v1.json). Independently review the JSON's Linux/tool/interpreter/import versions, consumed paths, overlay/binary digests, reproduced model identities, measured observations and native receipt before copying all seven concrete pin fields (seven resource identities) into the compiled profile. The parameters digest hashes the original reference file; the owned supervisor only rewrites the walk policy locator in private copies and checks their bytes throughout readiness.

Hashing matches production: SHA-256 over raw file bytes; the Python executable follows its canonical symlink target. Venv SHA-256 covers compact UTF-8 JSON of a sorted map of relative POSIX paths: files map to content hashes, and directory aliases map to `["directorySymlink", literal link text, canonical root-relative target]`. Relative directory symlinks such as `lib64 -> lib` are accepted only when their canonical targets remain inside the same canonical venv root. Absolute directory links, escapes, broken links and cyclic directory graphs (including ancestor/sibling cycles) are rejected. The physical target tree is hashed once, not traversed again through aliases; changing/removing an alias changes its identity. `__pycache__` and `.pyc` file contents remain excluded, but directory aliases there are still validated and identified. File symlinks retain target-byte hashing, including the standard external Python executable link. [Shared golden fixtures](../../scripts/fixtures/microduck-environment-digest-v1.json) are independently checked in Python and by Rust's production hashing function. Environments without directory aliases retain their existing digest. Environment pins identify the actual installation, including its paths/bootstrap files; another host must establish/review its own exact installation rather than copy unrelated hashes.

Current status: **velstand configuration and artifact bytes verified from pinned upstream; real readiness and Stage 9B qualification pending.** Run `008` remains historical evidence for the alpha pair and unchanged runtime resources only. The new exact parameter/policy pins are recorded, but promotion to `READY_FOR_QUALIFICATION` requires fresh readiness evidence. No qualification record, released binding or Accepted consequence is created. This migration did not run the ignored real Stage 9B probe.

Migration validation (2026-09-29): 35 deterministic Rust Stage 9 tests passed with the real probe ignored; 71 Python tests passed (37 shared provisioning, 21 qualification producer, 12 environment/readiness, one owned preparation). Formatting, exact template/parameter/policy hash checks and local documentation links passed. The Rust suite completed with local IPC access after an interrupted sandboxed run. These are local regression/source checks, not MuJoCo/PPO or Linux readiness evidence.
