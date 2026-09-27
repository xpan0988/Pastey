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

The macOS host lacks Linux bubblewrap, the pinned Python 3.12 stack, MuJoCo/ONNX Runtime and exact PPO artifacts. No real simulator/controller qualification or production release record exists. Rust/native-oracle and Python orchestration tests exercise mechanism/qualification plumbing only; they cannot fill the missing golden identities or authorize release. No hardware qualification is performed.
