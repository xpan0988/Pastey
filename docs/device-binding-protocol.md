# Device binding protocol

A device binding is the only place Pastey knows *how* a device works. Physical Core owns capability discovery, bounded authorization (root, grant, narrowing), lifecycle (session, lease, epoch, fence), cross-device scheduling and consequence adjudication. A binding owns payload schemas, completion and handover parameters, witnesses, native I/O and the device-side runtime. This document states what a binding must provide and what its device-side runtime must guarantee.

Requirement keywords (MUST, MUST NOT, SHOULD) are normative. Section 3 is the fence contract. Section 5 maps it to the conformance tests that exercised it on the first, since removed, device runtime.

## 1. Boundary

- Core sees only opaque capability and contract IDs, schema digests, canonical parameters, bounded dimensions (`BoundSetV1`), implementation fingerprints and witness classes. It never decodes a payload or a measurement.
- A binding holds no authority. It cannot create a root, grant, session, action or acceptance. It receives Core-built views and returns data that Core revalidates.
- An acknowledgment is not a physical consequence, and a physical consequence is not task acceptance. A binding's `accepted` reply, install evidence or fence acknowledgment never makes an action complete; only a registered witness's verdict, admitted by Core, can verify a consequence, and only Core's L7 decision accepts a task.
- Simulation evidence never supports hardware. A binding that reports its own facts (`SelfDescribed` provenance) can prove only a simulation environment and qualify only `AdapterIsolationOnly`.

## 2. The `EnvironmentBinding` trait

Defined in `src-tauri/src/physical/control.rs`. Core calls a binding; a binding never calls Core. Every future may complete with `None`, meaning the disposition is unknown: Core records it as unknown, closes the root and quarantines the session. It never retries.

| Method | Binding returns | Core does with it |
|---|---|---|
| `describe(host)` | Enrollment record, provenance and conditions digests, opaque implementation fingerprint | Enrolls and resolves a sealed `EnvironmentBindingV1`; checks qualification digests and fingerprint equality. A self-described binding is simulation-only |
| `validate_scope(fields)` | `Ok` or a rejection | Runs after Core's own validation on every review, approval, start, grant and policy change; never cached. Reject-only: it cannot rewrite a scope |
| `status()` | `Ok` while the device side is live | Checked on every use of the sealed binding and on every sample; an error makes the binding unusable |
| `witnesses()` | Witness per completion/handover contract ID | Each must match, by class, the registry Core was started with, or qualification fails closed |
| `install_session(view)` | Session enforcement evidence for exactly the view's session, epochs and request | Activates the session only if the class meets the reviewed minimum. `NativeFence` evidence is refused: no receipt verifier exists |
| `apply(view)` | Write receipt for exactly the view's session, epochs, request, action, option and payload digest | Records `accepted`/`refused`, even when the receipt arrives after the action's deadline (then also as a late callback). One that arrives after the action closed leaves the action `dispatch_unknown` and is kept only as history. Neither revives anything. Anything else, a mismatch or a missing reply, is unknown and closes the root |
| `fence(view)` | Fence evidence for exactly the stored fence request | Records the fence acknowledgment only; the consequence stays unknown until a witness decides |
| `evaluate_start(predicate)` | `Ok` only if the device is in the capability's start state; read-only | Runs before installation; a rejection means the session never starts |
| `observe()` | One control observation for the installed session, sealed observations and dispositions produced since the previous sample, and an opaque view for a brain | Checks source/body/world incarnations, capture order, age, gap and identity replay; evidence lineage must match the stored action. The view is passed to the brain untouched |

A decision-stream capability declares its options in the descriptor: each option is a name plus the digest of a fixed payload the binding holds, together with the shortest interval between decisions the binding supports. Core admits a decision by option name and digest; the apply view carries the option name and digest, and the binding must verify the digest against its payload before writing.

Views carry a validity handle (Core flags, clock and deadline). A binding SHOULD check it immediately before any native write and MUST NOT write once it no longer allows. The binding enforces the action's deadline itself. Core records exactly what the receipt says, whenever it arrives, and decides nothing from a late receipt beyond recording it. A write accepted after its deadline, or after Core closed or fenced the session, breaks this contract: Core keeps it as evidence of the violation, never as authorization.

A binding reads a view only through its accessors (session, epochs, binding, action, option, payload digest, evidence lineage, deadline, `allows`). It answers only with replies built from the view it answers: `isolated()` for an installation, `receipt(accepted)` for a write, `fenced()` for a fence. Core revalidates every field regardless. Evidence enters as a producer's sealed observations and dispositions in the next sample. `src-tauri/src/physical/bindings/sim.rs` is a reference runtime: a simulated device side that meets the requirements below, used by the physical demo.

## 3. Device-side runtime requirements

These requirements apply to the runtime that consumes commands next to the actuators: a daemon, firmware task or simulator process. They must hold without Pastey: a crashed binding process, a lost network or a dead Host must still leave the device stopped.

### 3.1 Identity and incarnation

- **R1.** The runtime MUST generate a fresh, unpredictable controller incarnation on every process start. It MUST NOT restore task state (installation, action, pending command, sequence) from before the start.
- **R2.** The runtime's identity (environment, conflict domain, body reference, body, world, controller incarnation) is fixed when it is configured. An install carrying any other identity, protocol or profile MUST be rejected.
- **R3.** A body, world or controller replacement MUST surface as a new incarnation, so Core rejects installs and observations bound to the old one.

### 3.2 Epochs and installation

- **R4.** The runtime MUST keep one monotonic high-water epoch per conflict domain. An install whose epoch is at or below it MUST be rejected as stale. Epoch 0 and epochs above `i64::MAX` MUST be rejected.
- **R5.** An exact duplicate of the current install from its current owner MUST be idempotent. It MUST NOT extend the lease.
- **R6.** A newer install from the same owner MUST supersede the old one and drop its action and any pending command.
- **R7.** One connection owns a live session. Another connection MUST NOT install over it or take it over.
- **R8.** A lease deadline MUST lie in the future and at most the runtime's maximum lease from now; otherwise the install is rejected.

### 3.3 Actions and commands

- **R9.** No command MAY reach the controller before an action is admitted under the current install.
- **R10.** An admitted action has one exact identity, payload digest and deadline. Its deadline MUST lie in the future, within the runtime's maximum action duration and within the lease. A changed action under the same session MUST be rejected.
- **R11.** Every command MUST match the admitted action and payload exactly and carry a strictly increasing sequence. A refresh MUST NOT extend the action deadline, change the payload or admit a second action.
- **R12.** An invalid command from the current owner MUST close that owner's task window. An invalid command from any other connection MUST NOT affect the owner's valid authority.

### 3.4 Local self-stop

- **R13.** At the action deadline, at the lease deadline, or when no valid refresh arrived within the runtime's refresh-loss interval, the runtime MUST stop on its own: it applies its declared loss profile (for example zero command) without any request from Pastey.
- **R14.** A closed action MUST NOT resume under the same action identity, even if a later refresh arrives.
- **R15.** The control loop MUST NOT wait on IPC. If task state is contended or poisoned, that tick applies the loss profile.

### 3.5 Fence

- **R16.** A fence names the current install and a next epoch strictly above the high-water epoch. The runtime MUST advance the high-water epoch to it, close the window and clear any pending command. Stale or foreign fences MUST be rejected; an exact duplicate fence from the owner is idempotent.
- **R17.** The fence MUST serialize with the actuation critical section. Once the fence is acknowledged, no command from the fenced session reaches the actuators. A command received before the fence but not yet consumed MUST NOT reach the controller.
- **R18.** A fence racing a refresh MUST leave the old action closed, whichever arrives first.
- **R19.** After a fence, delayed commands and replays MUST be rejected. A delayed acknowledgment is inert data and MUST NOT change runtime state.
- **R19a.** After a fence the runtime SHOULD keep producing sealed evidence of the body, never commands, and report a `fenced` disposition naming the fence request. A safe handover rests on that trace; without it the domains stay quarantined.

### 3.6 Connection and device loss

- **R20.** Loss of the owning connection MUST clear any pending command and close the window. Reconnecting MUST NOT resume the old session or reinstall its epoch; a fresh install at a higher epoch is required.
- **R21.** Controller, write or I/O loss MUST latch. No request, epoch or reconfiguration clears the latch; only a fresh process start (new incarnation, R1) does.

### 3.7 Wire hygiene and receipts

- **R22.** Unknown fields and unknown variants MUST fail to parse. A partial frame MUST NOT change state; transport loss while a partial frame is buffered closes authority (R20).
- **R23.** Every reply SHOULD report identity, request, accepted and reason, current install and action, command sequence, high-water epoch, fenced flag and last consumed sequence, so a binding can build exact evidence. A reply reports what the runtime did to its own state. It is not evidence of a physical effect.

## 4. What Core guarantees in return

- Epochs come from Core's durable domain ledger; each session reserves a strictly higher epoch, and a stale snapshot denies reservation.
- Revocation closes Core-side authority (RAM, then ledger) before the fence request is sent. A failed or unknown fence leaves the session quarantined and its consequence unknown; nothing continues after recovery.
- Budgets are reserved at admission and consumed at dispatch; loss, refusal or unknown dispositions never replenish them. Narrowing can only shrink bounds, durations and freshness.
- Core never retries an uncertain install, apply or fence under the same authority.

## 5. Conformance material

The requirements above were exercised by the task-authority tests of the first device runtime and by its process tests against a running daemon, both at git tag `pre-physical-decouple`. A new device runtime SHOULD port these to its own tests.

| Requirements | Former overlay test |
|---|---|
| R4, R5 | `first_and_duplicate_exact_install_are_idempotent` |
| R4, R7 | `stale_epoch_foreign_connection_and_domain_are_rejected` |
| R6 | `newer_same_owner_install_supersedes_old_commands` |
| R2, R8 | `wrong_identity_protocol_profile_and_bounds_reject_install` |
| R11 | `exact_action_refresh_keeps_original_deadline`, `changed_payload_action_session_epoch_deadline_and_sequence_reject` |
| R9 | `no_move_before_native_action_admission` |
| R13 | `action_and_session_expiry_reject_without_pastey` |
| R13, R14 | `refresh_loss_closes_and_cannot_resume_same_action` |
| R17 | `received_command_fenced_before_consumption_never_reaches_controller`, `fence_serializes_through_actual_apply_critical_section` |
| R16, R19 | `fence_then_delayed_command_and_duplicate_fence_remain_closed` |
| R18 | `fence_racing_refresh_always_leaves_old_action_closed` |
| R20 | `disconnect_and_reconnect_cannot_resume_or_reinstall_old_epoch` |
| R21 | `body_controller_io_loss_requires_fresh_launch` |
| R1, R3 | `restart_has_new_incarnation_and_no_buffer_or_authority` |
| R22 | `unknown_fields_and_variants_do_not_parse`, `partial_install_and_fence_never_mutate_authority` |

The Python process tests covered the same properties against a running daemon: install/consume/refresh, delayed ACK and replay after fence, expiry without Pastey, missing refresh, lease expiry without an action, disconnect/partial fence/reconnect, controller restart and invalid owned commands.

## 6. Decision streams

A brain reaches a stream only through the executor's tool dispatcher: one tool per approved option, `observe` and `remaining_budget`. Every decision is a new proposal: a fresh sample, a fresh challenge, then admission (option approved and declared, digest, rate, per-action duration, cumulative count and time), then one write. Admitting a decision fences the previous one: its validity closes first, then one ledger transaction closes it and admits the next. A tool result is only allowed (with the binding's native disposition) or refused (with a reason); it never reports a physical consequence. Every proposal is recorded with its caller, allowed or refused.

The stream ends when the witness verifies the completion contract (Core accepts and fences), when the budget is spent, or when authority closes: closing the tool session, losing the bridge route (indistinguishable on the executor from a crashed brain), losing the binding, or revocation. After a close the outcome is uncertain unless already verified, and nothing resumes.

## 7. Open points

- **NativeFence.** No binding can supply a verifiable native receipt, so `NativeFence` installation, command and fence evidence fail closed. A future receipt format needs a binding-supplied verifier whose checks Core can replay from the ledger.
