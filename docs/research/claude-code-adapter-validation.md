# Claude Code as the second native capability: adapter architecture validation

Status: validation, 2026-11-04, on `main` at `32065f5` (after 2.0.0). Diagnosis only: no production adapter, Core, Room Control or renderer behaviour changed. Added: the probe in [`tools/claude-code-adapter-probe`](../../tools/claude-code-adapter-probe/README.md) and one ignored test that pins a Core gap. **A follow-up the same day fixed the Core findings: see [section F](#f-follow-up-2026-11-04-core-corrections).** Sections A–E are left as first written, except where marked.

The question: can Claude Code be a native Host capability without changing Pastey's Core task lifecycle, authority, Bridge, recovery or execution contracts?

Evidence levels:
- **Measured**: run on this Mac against the installed Claude Code 2.1.292.
- **Documented**: Claude Code's documentation (code.claude.com, read 2026-11-04) or the CLI's own `--help`.
- **Code**: Pastey's source or tests at `32065f5`.
- **Inferred**: reasoned, not observed.
- **Not verified**: not established either way.

**Limit:** the installed CLI is signed out (`claude auth status`: `loggedIn: false`), and Pastey must not sign in for the user. So every model request failed at once. Session identity, refusals, cancellation and process containment were measured on real `claude` processes, but no model turn ran. A per-run hook stood in for a running tool. Section C lists what still needs a signed-in run.

## A. Verdict: CONDITIONALLY VALID, for local new sessions only

Claude Code fits the lifecycle when the adapter starts its **own** sessions with `claude -p` (stream-json) in a folder on its own Host, under three conditions:
1. The adapter contains the process itself. `claude -p` does not stop when Pastey dies (measured), and Core's restart rules assume it does.
2. Core's folder occupancy becomes capability-neutral (one generic change; a failing test proves the gap).
3. The renderer gets a capability-neutral start path (a product gap only).

This verdict is provisional until the model-turn checks in C pass.

Not validated:
- **Remote continuation of an existing Claude Code session.** No official interface gives both writer exclusion and containment (E1).
- **Remote new sessions.** Not established: Windows was not run, and shared-folder resolution is Codex-only (C2).
- **Approval forwarding.** Not established.

What held unchanged: task identity and replay, admission (Review and Full Access before any adapter sees input), ownership, conversation binding, the opaque bounded payloads, the outcome kinds, Stop's authority-first order, Burn, and the text output shape.

What looked generic but is Codex-shaped:
- the restart rule;
- folder occupancy;
- shared-folder resolution.

## B. Evidence matrix

| Stage | Pastey guarantee | Claude Code behaviour (level) | Result | Remaining uncertainty |
|---|---|---|---|---|
| Discovery | Bounded probes; Available / Incompatible / Unavailable with a shareable reason | `claude --version`, `claude auth status` (signed in or not, no secrets), `claude agents --json` (Measured). The `initialize` control request answers before any prompt but omits the interrupt capabilities; `system/init` lists them only after the first prompt (Measured). | Fits; adapter-local. Gate by version, as Codex is. Signed out → Unavailable with a reason. | The oldest version with every behaviour relied on |
| Invocation | Opaque bounded input; Pastey picks no provider, model, auth or sandbox | `-p --input-format stream-json --output-format stream-json --verbose`; the user's settings, hooks, MCP and CLAUDE.md load. `--bare` reads only an API key (Documented), so the adapter must not use it. A parent Claude Code process's variables (`CLAUDE_CODE_SIMPLE`, `ANTHROPIC_BASE_URL`, host sign-in refresh) change a child's auth (Measured). | Fits; adapter-local. Strip only host-coupling variables. | `-p` skips the workspace-trust dialog, so a folder's project hooks and `.mcp.json` run untrusted (Documented; measured in F5). No official way to read Claude Code's trust state. |
| Session | Owner + capability + scope; exact continuation or a definite non-start; never a substitute | `--session-id U` creates exactly `U`; reusing `U` is refused (exit 1, no events); `--resume U` keeps `U`; an unknown id gives `No conversation found`, no `init`, exit 1 (Measured). | Fits unchanged. The continuation payload is the session id, chosen by Pastey before launch. | Context continuity across resume (needs a model turn) |
| Completion | DONE only from exact native success; output never decides it | Final `result`. An auth failure came as `subtype: "success"` with `is_error: true`, plus a synthetic assistant message (Measured). Background work can start more turns after a `result`; the process waits up to 10 min idle for it (Documented). | Fits with adapter rules. Completed means the last `result` was `success` with `!is_error` **and** the process exited after stdin closed. The folder stays held until exit. | Extra turns not measured |
| Cancellation | Authority revoked first; native stop best effort; occupancy held until the observer exits | Interrupt control request: acknowledged in ~0.1 s, child killed, `error_during_execution`. SIGINT: the same, exit 0. SIGTERM: exit 143 with **no record**. stdin EOF: **does not stop** (Measured, hook stand-in). | Fits; adapter-local. Stop sends the interrupt, with SIGINT as fallback. | A real Bash tree, background commands and subagents under interrupt |
| Restart / crash | Restart never resumes a turn; an unbound local task fails and frees the folder; a Bridge task needs reconciliation (native_agent.rs:1352–1383) | SIGKILL of the controller: `claude` stayed `busy`, finished the turn ~75 s later, then exited. Under a lifeline watchdog: interrupted within 1 s, gone within 5 s, nothing left (Measured). | **Gap.** Core assumes native execution dies with Pastey. Codex's app-server does (docs/layers/layer-5-agent.md:404); Claude Code does not. Fixed adapter-locally by the watchdog, but the contract never states the obligation (C1). | Windows (Job Object) not run; a restart inside the watchdog's grace window |
| Ownership | One owner per conversation; peers never see native ids; Burn forgets | Per-OS-user store `~/.claude/projects`, UUID-named; Pastey picks the UUID (Measured). | Fits unchanged (`NativeConversationBindingV1`). | Pastey's sessions show in the user's Claude history and can be opened there, as Codex threads can |
| Exclusivity | One unresolved invocation per resource; one canonical folder owned while anything can affect it | `-p` has **no writer lock**: a second `-p --resume` of a held session was accepted, and both wrote, forking the transcript (Measured). The registry lists `-p` processes (`interactive`, `busy`/`idle`) (Measured). `--bg --resume` copies a held session (repo evidence, 2.1.292). | Pastey against Pastey: fits through Core keys, once they are generic. Against other clients: detectable, not preventable (E2). Across providers: Core gap (C2), proven by test. | Behaviour while the holder is mid-turn |
| Permissions | Pastey never answers by itself; only an explicit human decision is relayed | With no host, `-p` denies anything that would prompt, and the turn goes on (Documented). A host is either `--permission-prompt-tool` (an MCP tool) or the Agent SDK's own `stdio` wiring, which the CLI docs do not list. `--bg` waits for `claude attach` (Documented). | Local: fits as native behaviour. Relay: Core's decision kinds (command, file change, unsupported) fit Bash / Edit / Write, but this is **not established**. | Nothing measured (needs a model turn). Whether a stream-json client without `stdio` counts as a host |
| Remote admission | Review or Full Access is the only admission; no peer protocols means no Bridge invocation (native_agent.rs:4182) | — | Fits unchanged; invoke-v2 is generic. | — |
| Recovery | Durable envelope facts; reconciliation never reruns | The transcript records each step, interruptions and API errors. SIGTERM records nothing (Measured). | Fits once contained. Transcript reconciliation would be an optional adapter hook. | — |
| Output | Opaque, ≤ 16 KiB, shared text shape | `result.result` text (Documented). | Fits unchanged (`text_output`). | — |
| UI | Capability-neutral task projection | — | Status, Stop, Review, decisions, recovery and output are generic. Start and availability are Codex-only (D1). | — |

## C. Prototype results

Environment: MacBook arm64, macOS 26.5.2 (25F84). Claude Code 2.1.292, native install (`~/.local/bin/claude` → `~/.local/share/claude/versions/2.1.292`), signed out. Python 3.14.7. Probes ran outside the sandbox, from a clean environment: `env -i HOME USER LOGNAME PATH SHELL TMPDIR LANG python3 -I tools/claude-code-adapter-probe/probe.py <experiment>`. Every run used a new empty temporary folder. Per-experiment output is in the [probe README](../../tools/claude-code-adapter-probe/README.md#evidence-2026-11-04-macbook-arm64-macos-2652-25f84-claude-code-21292).

| Check | Result | Basis |
|---|---|---|
| 1. Detect the runtime | Pass: version, registry, sign-in state | Measured |
| 2. Start a new session | Pass: the chosen id is honoured, a duplicate id is refused | Measured; the model call failed (signed out) |
| 3. Bounded final response | **Not verified**: no model turn. The terminal `result` shape was verified, including `success` with `is_error: true`. | Measured shape |
| 4. Resume exactly the same session | Pass: same id; an unknown id is a definite refusal | Measured |
| 5. Cancel and verify termination | Pass for a stand-in child: interrupt and SIGINT end the turn and kill the child; SIGTERM leaves no record; stdin EOF does not stop | Measured with a hook stand-in |
| 6. Success / failed / cancelled / indeterminate | Distinguishable. Success: `success` and `!is_error`. Failed: `is_error`. Cancelled: `error_during_execution` after the adapter's own acknowledged interrupt. Definite non-start: *corrected in F5:* not "no `init`", since a project `SessionStart` hook runs before `init`; only evidence that nothing began. Indeterminate: the stream ends without a `result`. | Measured (except a real success) |
| 7. Session / workspace exclusivity | Claude Code does not prevent two `-p` writers (fork measured). Pastey's Core does not hold a folder across providers (failing test). | Measured + Test |
| 8. The controlling process disappears | Without a lifeline, the turn runs to its end unobserved. With the watchdog, it is interrupted within 1 s and nothing is left. | Measured with a hook stand-in |

Tests, outside the sandbox: `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey capability::claude_code` gave 8 passed and 2 ignored. The new `a_running_continuation_holds_its_folder_against_every_workspace_capability` (`--ignored --exact`) **fails as designed**: while a Claude Code continuation runs in a folder, `workspace_has_authoritative_owner` reports the folder free. (F: now active and passing.)

Needs a signed-in CLI (`claude auth login`, by the user):
- a bounded reply and context continuity;
- interrupt, SIGTERM and orphaning of a real `Bash(sleep 60)` (its tree, background commands);
- exclusivity while the holder is mid-turn;
- permission requests with no host and with the `stdio` host;
- extra turns after a `result`.

Windows was not run at all.

## D. Architecture findings

**A. Provider-local** (inside a new adapter; no contract change):
- A1. Launch `claude -p` with stream-json in and out, `--verbose`, and `--session-id U` (new) or `--resume U` (continue). Pass no model, permission, settings, `--bare` or tool flags. Strip only the variables that couple a child to another Claude Code host.
- A2. Outcome mapping: as in check 6. A synthetic API error is Failed, never Completed. The existing transcript classifier already treats `isApiErrorMessage` as failed (`sessions/claude.rs:588`).
- A3. Stop sends the interrupt control request (acknowledged), with SIGINT as fallback. Never SIGTERM as Stop (it records nothing), and never stdin close alone.
- A4. Containment: a lifeline (the watchdog above on macOS and Linux; a Job Object without breakaway on Windows, as `crates/codex-utils-pty/src/win/job.rs` does) keeps a turn from outliving Pastey. Holding an advisory lock per conversation for the turn's life would also refuse a restarted Pastey until the old turn is gone, much like Codex's writer lock (Inferred).
- A5. Gate on `claude --version` and on signed-in state before each new session.

**B. Generic contract gaps** (provider-neutral extensions to `NativeCapabilityAdapterV1`):
- B1. The trait (`capability.rs:234`) never states that a run's native execution must not outlive Pastey's process. Core's restart rule depends on it. State it, so an adapter that cannot meet it is not registered as if it could.
- B2. Exclusivity is one key, namespaced by capability (`ExclusivityKeyV1::new`, `capability.rs:118`), so no adapter can claim a folder that another capability also uses. Fix: a Core-owned folder key, derived from each adapter's `host_workspace` (`capability.rs:132`), held beside the adapter's own key.
- B3 (optional). An adapter reconciliation hook at restart, to read the transcript of an interrupted turn. Codex has none either.

**C. Core abstraction leaks** (Core depends on Codex semantics):
- C1. **Restart assumes Codex's process model.** For an unbound local task, `restore_durable_envelopes` records `failed` / `native_agent_interrupted_on_restart`, deletes the envelope and frees the key (`native_agent.rs:1352–1383, 1664`). Correct for Codex, whose app-server ends with Pastey. The shipped `agent.coding.claude_code` (`--bg`, `claude_code.rs:445`) runs under Claude Code's supervisor and outlives Pastey. After a restart, a task reported failed may still edit the folder and complete. The adapter's own registry check prevents a second write, but the recorded fact is wrong (`remote-continuation.md` notes the lost outcome; Core does not). Fix: B1 + A4.
- C2. **Folder occupancy is Codex's key.** `workspace_has_authoritative_owner` checks `capability::codex::workspace_exclusivity_key` (`native_agent.rs:4955`), as does movement terminal cleanup (`:2997`). Active tasks are matched by key only (`:4967`). Claude Code keys by session and reports no `host_workspace` (`claude_code.rs:238–243`). So a running Claude Code task does not hold its folder against workspace movement, a result Apply, or a Codex task there. Proven by the failing test. Fix: B2.
- C3. **Shared-folder resolution (a WHERE authority) dispatches on Codex IDs.** `resolve_agent_request` returns `None` for any other capability (`session_read_access.rs:582`), and Room Control passes that input through unchanged (`room_control.rs:2033`). A non-Codex capability therefore cannot use folder references or a share's Agent requests switch, and its Review shows no folder. This is not a bypass (Review and Full Access still gate), but remote parity needs a provider-neutral scope field or an adapter hook.
- C4. **Workspace movement and v1 invoke are Codex-only by construction** (`native_agent.rs:184, 1102, 1806, 2419–2429`). This is declared in the docs and not needed for local Claude Code.

**D. Product / UI gaps:**
- D1. Availability is hard-coded to `agent.coding.codex` in `AgentWorkspace.tsx:86–87`, `SettingsScreens.tsx:207` and `DeviceDetails.tsx:172`.
- The local start goes through `startNativeCodexTask` (`lib/tauri.ts:303`) to `start_native_codex_task`; the remote start through `startRemoteNativeCodexTask` (`:342`). `start_remote_native_capability_task` exists but the renderer never calls it.
- `remote_native_capabilities` names only Codex (`native_agent.rs:7219`).
- Already generic: status, Stop, Review, native decisions, recovery, display names (`AgentTaskLifecycle.tsx:162–164`) and Markdown output (`:103`).

**E. Fundamental incompatibilities** (with Claude Code's current official interfaces):
- E1. **Continuing a session another client may hold, locally or remotely.**
  - `-p --resume` has no writer exclusion (fork measured).
  - `--bg --resume` refuses a held session (it copies instead) but runs under Claude Code's supervisor. That supervisor outlives Pastey and restarts sessions that exit unexpectedly (Documented), so Pastey cannot contain it.
  - No interface gives both. Remote Claude Code continuation should stay off.
- E2. **Writer exclusion against non-Pastey clients for Pastey's own sessions.** It can be detected (registry, transcript fork) but not prevented. Codex 0.163+ prevents it with its writer lock.

## E. Smallest implementation plan

Essential, in order:
1. With the CLI signed in by the user, run `cancel`, `orphan`, `exclusivity` and `permission` from the probe. Stop if a real Bash tree survives interrupt or the lifeline, or if a `result` is followed by more work without the process staying alive.
2. Core, generic: make folder occupancy capability-neutral (B2, C2), un-ignore the test, and state the containment obligation on the trait (B1).
3. Adapter `agent.coding.claude_code_session`: new and continued sessions in a local folder, no peer protocols, built from A1–A5. Use a stream-json fixture `claude` for tests: success, `is_error`, interrupt, crash with no `result`, no `init`, a lingering process after a `result`, and lifeline kill.
4. Renderer: a capability-neutral local start (`start_native_capability_task(capability, input)`) and availability by capability.

Not now; each is its own decision:
- remote new sessions (peer protocols, C3, a Windows Job Object and physical Mac ↔ Windows acceptance);
- approval relay through the `stdio` host;
- the reconciliation hook (B3);
- non-Codex workspace movement (C4);
- correcting C1's recorded fact for the existing `--bg` continuation.

## Final questions

1. **Did the second provider show Pastey's Core is provider-neutral?** Mostly, not fully.
   - The authority and lifecycle core took Claude Code unchanged.
   - Three Core behaviours are Codex's: restart semantics, folder occupancy and shared-folder resolution. Each was hidden by having only Codex.
   - The fake adapter could not reveal them: it is in-process, has no folder, and never outlives the test.
2. **Which generic-looking assumptions were Codex-specific?**
   - That native execution ends with Pastey's process.
   - That the provider enforces one writer per session (Codex's lock).
   - That "the folder" means Codex's workspace key.
   - That shared folders are resolved by the adapter Core happens to know.
   - That a protocol-level "completed" settles the outcome. Claude Code reports `success` with `is_error`, and can keep working after a `result`.
   - That an unanswered approval blocks the turn. Headless Claude Code denies it and goes on.
3. **Can Claude Code be a first-class remote native Agent without weakening the guarantees?**
   - Remote new sessions: plausibly, once containment is proven on Windows, C3 is closed, and approvals are either left to Claude Code's native denial or relayed through an established interface. None of that is established today.
   - Remote continuation of existing Claude Code sessions: no, not with the current official interfaces (E1).
4. **The smallest technically justified next step:** sign the CLI in and run the four model-turn probe experiments, because the containment and completion conclusions still rest on a hook stand-in. Then make the one Core change, capability-neutral folder occupancy, with its test.

## F. Follow-up, 2026-11-04: Core corrections

Scope: the proven Core defects only.
- No production `-p` adapter.
- No remote Claude Code.
- No Bridge, transport or identity change.
- No multi-Agent coordination.

Nothing here serializes Agents in general. Hosts are independent, and so are unrelated folders on one Host.

### F1. Root causes (proven defects)

1. **Folder occupancy was Codex's key.**
   - `workspace_has_authoritative_owner` and movement cleanup looked up `capability::codex::workspace_exclusivity_key`.
   - Running tasks were matched only by exact key, and keys are namespaced by capability.
   - Claude Code keyed by session and reported no `host_workspace`. So a running Claude Code task held nothing that a Codex task, a movement approval or an Apply would see.
   - Paths were compared for equality, so a nested folder counted as another resource.
2. **Restart assumed every native process dies with Pastey.**
   - An unbound local running task became `failed` / `native_agent_interrupted_on_restart`, which the renderer shows as "…and it stopped with it". Its envelope was deleted and its folder and key released.
   - That holds for Codex: its app-server exits when its stdin closes. Measured with 0.163.0, it was gone within 0.5 s of its controller's SIGKILL.
   - It is false for Claude Code's `--bg`, which Claude Code's supervisor runs (measured), and unknowable for a capability that is no longer registered.
3. **A stop was reported as "Stopped" without evidence.**
   - The Claude Code adapter returned Cancelled after `claude stop` without checking.
   - Any run that ended Unknown after Stop kept `native_agent_cancel_requested`, which the renderer shows as "Stopped".
4. **A failed `--bg` launch counted as a definite non-start without evidence.** Any non-zero exit without the continuation message became "Did not start · nothing ran".

### F2. Changes

**Generic Core contract:**
- **Folder occupancy.** It comes from every adapter's canonical `host_workspace`, across capabilities. The same folder, or a folder inside or around it, is one resource (`workspaces_overlap`). It is held by observed runs (until the observer lets go), uncertain outcomes and movements.
  - Changed functions: `invocation_has_authoritative_owner`, `workspace_has_authoritative_owner` and `cleanup_terminal_remote_workspace` in `native_agent.rs`.
  - The adapter's own key now stands only for native-session exclusivity.
- **Containment declaration.** New `NativeCapabilityAdapterV1::native_execution_ends_with_pastey()`, default `false`. Codex, the Codex thread adapter and the test fake declare `true`.
- **Re-observation.** `StartedInvocationV1::recovery` and `NativeCapabilityAdapterV1::reobserve()`. Core keeps the record Host-private in the envelope (`native_recovery`). At restore it observes again through the same observer, now extracted unchanged as `observe_invocation`.
- **Restore rule.**
  - A running (or already unknown) local task whose capability does not end with Pastey becomes `interrupted` / `native_agent_outcome_unknown`; a Bridge-owned one keeps `native_agent_reconciliation_required` as before. Either keeps its envelope, folder and session key, and is re-observed when the adapter can.
  - Contained capabilities are unchanged.
- **Unconfirmed stop.** When Stop's run ends Unknown, the task records `native_agent_cancel_delivery_uncertain` ("Stop requested · not confirmed").

**Provider-specific (the shipped Claude Code `--bg` continuation adapter):**
- reports its canonical folder;
- records `(session, folder, prompt, start time)` for re-observation;
- re-observes from the registry and transcript, never launching again;
- counts Stop as stopped only once the registry stops listing the session (15 polls);
- treats a failed launch as a non-start only when the registry holds nothing and the transcript has no entry for the prompt.

**Frontend:** no change needed. The existing wording for `native_agent_outcome_unknown` and `native_agent_cancel_delivery_uncertain` already says what is true.

**Safety restrictions introduced:**
- An unknown outcome is never auto-released by Core: only native evidence through `reobserve`, or this Host's user's explicit Stop, releases it.
- Nested folders now conflict. This is the one change visible with Codex alone: a Codex turn in `X` and another in `X/sub` ran at once before.

### F3. Concurrency matrix

| A | B | Result | Basis |
|---|---|---|---|
| Any task on Host A | Any task on Host B, even under the same path | Run at once | Test (two services) |
| Codex in `X` | Claude Code in a sibling `Y` (e.g. another worktree) | Run at once | Test |
| Codex in `X` | Claude Code in `X`, through a symlink alias, or in another case spelling | B refused before launch | Test |
| Codex in `X` | Claude Code or Codex in `X/sub` | B refused | Test |
| Claude Code in `X` | Codex in `X` or `X/sub` | B refused | Test |
| A task in `X/sub` | Approving a movement of `X` | Refused; the movement keeps awaiting approval | Test |
| Approved movement of `X` | Claude Code in `X` | Refused | Test |
| Pastey task in session `S` | Another Pastey task in `S` | Refused (session key) | Test |
| Pastey task in `S` | Another Claude Code client in `S` | **Not prevented by Pastey.** `--bg` refuses or copies at launch (measured); a later outside `-p --resume` writes concurrently (measured fork) | Measured |
| Unknown outcome (restored) in `X` | Anything in `X` | Refused until its end is observed or it is explicitly stopped | Test |
| Session discovery or inspection | Anything | Never occupies | Code |

Bridge, provider and session identity are not part of a folder's identity.

### F4. Crash and restart guarantees

| Capability | After a Pastey crash or restart |
|---|---|
| Codex, Codex thread (contained) | Unchanged. Local: `failed` / `native_agent_interrupted_on_restart`, released. Bridge: `native_agent_reconciliation_required`. The app-server's exit is measured; an in-flight turn's tool processes are not. |
| Claude Code `--bg` (not contained; recovery record) | `interrupted` / `native_agent_outcome_unknown`, with folder and session held. Re-observed: the outcome comes only from native evidence (registry plus transcript). A Stop is confirmed through the registry, or stays "Stop requested · not confirmed" and held. Never sent again. |
| Not contained, no recovery, or no longer registered | Unknown and held (fail-closed). Resolved only by an explicit Stop: "Stop requested · not confirmed", released on that decision (the existing rule for uncertain turns). |

**Definite non-start:**
- Codex: unchanged (refused before `turn/start`).
- Claude Code `--bg`: only with evidence that nothing began.

### F5. Workspace trust and work before `init` (measured)

- In a never-trusted folder, `claude -p` ran the project's `SessionStart` and `UserPromptSubmit` hooks and started its `.mcp.json` server (`source: project`), with no prompt.
- The `SessionStart` hook ran **before** `init`. So neither a missing `init` nor a missing `result` proves that nothing ran.
- `claude --bg` in the same kind of folder refused (`Workspace not trusted…`, exit 1), and ran no hook, no MCP server and nothing in the registry. The shipped adapter therefore inherits Claude Code's own trust check, and Pastey's admission never stands in for it.
- **Smallest safe boundary for any future `-p` adapter (proposed, not implemented):**
  - Executor Review or Full Access authorizes running the Agent, not trusting the folder.
  - Before launching, the adapter refuses (a definite non-start: nothing launched) a folder whose project configuration Claude Code would execute without asking: hooks or enabled plugins in `.claude/settings.json` or `.claude/settings.local.json`, and servers in `.mcp.json`.
  - Pastey cannot read Claude Code's trust decision, and `--bg`, which checks trust, cannot be contained. So refusal is the only safe default until Claude Code offers a headless trust check.

### F6. Tests

All tests below use fixtures in one process (a "Host" is one `NativeAgentServiceV1`). They are not physical multi-Host or real-provider evidence.

**New tests in `src/native_agent/tests/resource_occupancy.rs`** (real Codex and Claude Code adapters on fixtures):
- `codex_on_one_host_and_claude_code_on_another_run_at_once`: requirement 1.
- `codex_and_claude_code_on_one_host_run_at_once_in_separate_folders`: requirement 2, sibling worktree.
- `claude_code_is_refused_a_folder_codex_holds_by_any_name_or_nesting` and `codex_is_refused_a_folder_claude_code_holds`: requirement 3, including symlink, case spelling and nesting.
- `workspace_movement_respects_a_folder_claude_code_holds_and_holds_its_own_source`: requirement 4.
- `a_turn_no_registered_capability_vouches_for_stays_unknown_and_held_after_restart`: fail-closed restart.

**New tests in `capability/claude_code/tests.rs`:**
- `a_second_pastey_continuation_of_a_running_session_is_refused`: requirement 5, Pastey against Pastey only.
- `a_restart_while_the_session_runs_keeps_the_outcome_unknown_then_observes_its_end`: requirement 6.
- `stopping_a_task_restored_after_a_restart_stops_its_session_and_only_then_releases_it`.
- `a_stop_claude_code_does_not_confirm_stays_a_request_and_keeps_the_folder_held`.
- `a_launch_failure_after_the_session_showed_activity_is_unknown_not_a_non_start`.
- The former known-gap test is now active and passes.

**Mutation checks** (each reverted afterwards):
- Comparing folders by equality instead of overlap fails 3 of the 6 occupancy tests.
- Letting observed runs hold no folder fails the same 3.
- Treating every capability as contained at restart fails all 3 restart tests.
- The allowed-concurrency tests stay green in every mutation.

**Results** (outside the sandbox, Apple Silicon Mac, macOS 26.5.2):
- `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey`: **885 passed, 0 failed, 9 ignored**, Codex's tests included.
- On the final source, `cargo test --bin pastey native_agent`: 255 passed, 7 ignored. Before this change it was 243 and 8: 11 tests were added and 1 was un-ignored.
- The compiler gives the same 34 warnings before and after; none is new.
- The `installed_…` tests that need a signed-in CLI or Codex quota stay ignored.

### F7. Still needs a real Claude Code run or Windows

*Updated by F9:* the real-Claude-Code items in this list have since been run, signed in, on 2.1.317. The rest still stand.

- A real Pastey restart during a real Claude Code turn: unknown, then observed to its end, then released. Tested with a fixture only.
- `claude stop` confirmation and copy-on-held against a real background session, and the trust refusal through the adapter end to end.
- Everything in C that needs a model turn: a real Bash tree under interrupt or crash, approvals, extra turns after a `result`.
- Windows: `--bg` itself, folder identity (`canonicalize` uses `GetFinalPathNameByHandle`; case and junction aliases unmeasured), Codex's exit on a closed stdin there, and Job Object containment.
- Linux bind-mount aliases (canonical paths do not unify them).
- Re-observing a Bridge-owned task: generic, but no capability that can outlive Pastey has peer protocols, so it is unexercised.

### F8. Minimal next step

The CLI is signed out, and only you can sign it in:

```bash
claude auth login
```

```bash
claude auth status
```

`claude auth status` should report `loggedIn: true`. Then:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_continues_an_unheld_session_and_refuses_a_held_one -- --ignored --nocapture
```

```bash
env -i HOME="$HOME" USER="$USER" LOGNAME="$LOGNAME" PATH="$PATH" SHELL="$SHELL" TMPDIR="$TMPDIR" LANG=en_US.UTF-8 python3 -I tools/claude-code-adapter-probe/probe.py cancel interrupt
```

Run the probe's `orphan`, `exclusivity` and `permission stdio-host` experiments the same way. Then, in the app:
1. Continue a disposable Claude Code session.
2. Force-quit Pastey while the turn runs, and relaunch it.
3. The task should read "Outcome unknown", its folder should be refused to a Codex run, and then it should complete or be stopped from Claude Code's evidence.

After that, the production `-p` adapter per E, with a lifeline and the F5 boundary.

## F9. Signed-in results, Claude Code 2.1.317

Claude Code auto-updated from 2.1.292 to 2.1.317 during this work. Everything below was re-run on 2.1.317, signed in with claude.ai (Pro), on the same Mac, from a clean environment and outside the sandbox. The prompts were harmless, in disposable folders.

**Through Pastey's own lifecycle** — `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_ -- --ignored --nocapture --test-threads=1` (run from a clean environment): **4 passed**.
- **Continuation and a held session.** The existing live test passes with the corrections: the continuation completed from Claude Code's own records, and a session another process held was refused before anything was sent.
- **Pastey killed mid-turn.** A separate Pastey process (this test binary, running `installed_claude_code_crash_child`) was killed with SIGKILL while Claude Code ran the turn. Its database was then reopened by a new service:
  - The task was `interrupted` / `native_agent_outcome_unknown`, not failed.
  - Its folder and session were held: a second continuation was refused.
  - It was then observed to `Completed` with Claude Code's real reply.
  - The prompt appears once in the transcript, and the session and folder were released afterwards.
- **Pastey killed, then Stop.** After the same kill, Stop sent `claude stop` and recorded `native_agent_cancelled` only once Claude Code's registry no longer listed the session. The prompt was sent once.

**Through the probe** (`claude -p`, real model turns; details in the [probe README](../../tools/claude-code-adapter-probe/README.md)):
- **Bounded reply:** `success` with `is_error: false`. Resuming the same id recalled the earlier reply.
- **A failed result can follow real work:** a turn ended `error_max_turns` after a real Bash call had already run.
- **Background tool runs:** 2.1.317's Bash tool refuses a foreground `sleep`, and the model ran it as a background task.
- **Cancelling a real background Bash task:**
  - interrupt: acknowledged, `error_during_execution`, tool process killed;
  - SIGINT: the same;
  - SIGTERM: exit 143, no `result`, tool process killed;
  - **stdin EOF does not stop it:** `result: success` came after 7 s, then the process waited for the background command and **took another turn** about 60 s later (a `<task-notification>` and a new reply in the transcript).
- **Controller SIGKILL during a real tool call:**
  - without a lifeline, `claude` stayed `busy`, its tool kept running, it took another turn, and it exited after about 75 s;
  - under the lifeline watchdog, the tool was killed and `claude` exited within 5 s.
- **No writer exclusion:** a second `-p --resume` was not refused.
  - With an idle holder, both wrote and the transcript forked.
  - With a busy holder, the second client reported `success` with **zero turns and an empty result**, while its prompt was answered later in the shared session.
  - The mechanism is undocumented. New `queue-operation` and `session-owner` records appear in transcripts; they are not a lock.
- **Approvals:**
  - with no host, a Write was denied (`permission_denied`) and the turn went on;
  - with the Agent SDK's `stdio` host, the Write arrived as a `can_use_tool` request, the probe's Deny was honoured, and no file was written.
- **Trust:** unchanged from F5.

**What this changes:**
- **The completion rule for any future `-p` adapter is stricter than A–E said.** Completed requires all of these:
  - the last `result` is `success` with `!is_error`;
  - the run took at least one turn of its own;
  - the process exited after stdin closed.
- **Approval relay** through the `stdio` host works on 2.1.317. It is still not a documented CLI interface, so it is feasible but not established.
- **Claude Code updates itself.** Behaviour relied on must be gated by version and re-measured, as the Codex adapters do.

**Still open:**
- Windows (all of F7's Windows items).
- A restart of the real app through its UI: the tests kill a Pastey test process, not the app.
- Whether `claude stop` ends a `--bg` session's background commands when Pastey releases a completed turn: not measured.
- Re-observing a Bridge-owned task.
- Linux bind-mount aliases.

## G. Production local adapter, 2026-11-04

Claude Code is now a second native Agent of Pastey's conversation view, on this Host only and on macOS and Linux. It is `agent.coding.claude_code_conversation`, built from F and F9 with no new framework. Layer 5's "Claude Code conversations" describes it. In brief:
- `claude -p` runs with stream-json in and out, the user's own installation and settings, a session id Pastey chooses, and exact `--resume`.
- Completion needs the echoed prompt `uuid`, a `success` result for this session after at least one turn, and a clean process exit. The output is the last result.
- With no approval host, Claude Code denies what would prompt; Pastey answers nothing.
- A folder with untrusted project configuration is refused, unless Claude Code's own trust record covers it.
- Containment is a lifeline: Pastey's own executable is `claude`'s parent and interrupts it when Pastey dies.
- After a restart the turn is unknown and is observed again.
- It is gated on version 2.1.317 or later and signed-in state.
- Windows is refused until acceptance R passes.
- The renderer offers Codex or Claude Code for a new conversation on this device. A conversation keeps its Agent; Settings lists both.

What was verified, and how:
- **Fixture tests (Rust):** 14 adapter tests and 3 lifeline tests. The native Agent suites give 272 passed.
- **Full Rust suite:** 902 passed, 0 failed, 12 ignored (the live checks). Codex's tests unchanged.
- **Renderer:** 74 of 74 frontend integration tests pass; type-check and `vite build` are clean.
- **Live:** Claude Code 2.1.317, signed in, through `start_claude_code_conversation_task` (what the `start_native_claude_code_task` command calls) and the built `pastey` executable as the lifeline. Run with `installed_claude_code_conversation -- --ignored --test-threads=1`: 4 passed.
  - **Conversation:** a new conversation completed with its reply.
  - **Follow-up:** it continued the same session and recalled that reply. It ran in a folder of this repository only because Claude Code's own record trusts the repository.
  - **Stop:** recorded `native_agent_cancelled`, and no Claude Code process was left.
  - **Pastey killed:** a separate Pastey process was killed with SIGKILL during a turn. Claude Code's run ended **0.15 s** later through the lifeline, with nobody stopping it. The restored task was `native_agent_outcome_unknown`, then observed `interrupted` from the transcript, never `failed`. The prompt was sent once.
- **Not driven:** the app's UI with a mouse, and the Tauri IPC layer itself. The live checks call the method the command calls.
- **Not run:** Windows. Acceptance R is the boundary. *(Section H: Windows containment, identity and trust are now implemented and tested on Windows, signed in (H7); enabling it waits on the app steps of its acceptance.)*

## H. Windows, 2026-11-09

Scope: `agent.coding.claude_code_conversation` on Windows, local only. Not in scope: remote Claude Code, Layer 4 or Bridge authority, `--bg` continuation (now reported unavailable on Windows), UI.

Environment: Windows 11 Pro 10.0.26200, x86_64, Rust 1.95.0, from a shell with every Claude Code session variable removed. The only Claude Code on this Host is the Claude desktop app's own copy, **2.1.309**, under `%LOCALAPPDATA%\Claude-Data\…\claude-code\2.1.309\…\claude.exe`: not on `PATH`, and **signed out** when run on its own. So no model turn ran in H1–H6, and 2.1.309 is older than the measured minimum (2.1.317). H7 adds the signed-in run on the user's standalone 2.1.319.

Evidence levels used here:
- **Physical**: real Windows processes on this Host, in the product's own Job Object code.
- **Physical, fixture**: the same, with a fixture `claude.cmd` (PowerShell `-p` runs that speak stream-json) in place of Claude Code. Pastey's behaviour is real; Claude Code's is simulated.
- **Measured**: the real `claude.exe` 2.1.309, signed out.
- **Not run**: needs a signed-in Claude Code; covered by the [Windows Claude Code acceptance](../platform/windows-claude-code-acceptance.md).

### H1. Unix and macOS assumptions found

| Where | Assumption | Effect on Windows | Now |
|---|---|---|---|
| Conversation probe | `cfg!(not(unix))` refused Windows | Unavailable | Gated on `WINDOWS_ACCEPTED_VERSION` instead |
| Lifeline | Descriptor 3, `setpgid`, SIGINT/SIGTERM/SIGKILL | No containment at all | Job Object (`job.rs`) |
| Trust record | Keys compared as `\\?\C:\…` or `C:\…` | Claude Code writes `C:/…` (measured in `~/.claude.json`): a trusted folder with project configuration was always refused (fail-closed, but unusable) | Matched as spelled, with forward slashes, or by canonical identity |
| Working folder | The canonical, verbatim `\\?\C:\…` | Measured: Claude Code normalises it to `C:\…`, so no defect | Passes the plain form, as a terminal would |
| Probes, registry reads | `child.kill()` on timeout ends only the root | An npm `claude.cmd`'s Node child could outlive a timed-out probe; console windows would flash from the GUI | Each in its own Job Object, without a console window |
| `--bg` continuation | Registered on every platform | Never measured on Windows; prompt passed as an argument through `cmd.exe` for a `.cmd` | Reported `native_agent_platform_unverified` |
| Re-observation (all platforms) | Transcript read, then registry | See H5 | Fixed |

### H2. Claude Code 2.1.309 on Windows (measured, signed out)

- `--version`: `2.1.309 (Claude Code)`. `auth status`: JSON with `loggedIn: false`, `authMethod: none` (Pastey: `native_agent_signed_out`). `agents --json`: a list with `pid`, `cwd` (`C:\…`), `kind`, `sessionId`, `status`. None of the three leaves a process behind.
- `-p` stream-json, `--session-id U`: `system/init` for exactly `U`; the replay echoes Pastey's own prompt `uuid`, which the transcript records; signed out, the result is `success` with `is_error: true` and one turn (Pastey: Failed). Reusing `U`: `Session ID … is already in use`, exit 1, no events. Unknown `--resume`: `No conversation found`, an `error_during_execution` result with 0 turns, exit 1.
- The interrupt control request is acknowledged (`control_response`, `success`) within 0.3 s. Its effect on a running turn was not seen (no turn ran).
- 2.1.309 also emits `command_lifecycle` events; the adapter ignores them.
- The transcript folder is named from `C:\…` (`C--Users-…`), as Pastey's `project_dir` computes.

### H3. Containment design

- `claude` is created suspended (`CREATE_SUSPENDED | CREATE_NO_WINDOW`), assigned to a new Job Object, then resumed. A process that cannot join is terminated before it ran (a definite non-start).
- Limits: `KILL_ON_JOB_CLOSE`, and neither `BREAKAWAY_OK` nor `SILENT_BREAKAWAY_OK`. The job is unnamed and its handle not inheritable: Pastey's process holds the only handle, so its end, however it ends, ends every process in the job.
- Stop: Claude Code's interrupt control request on stdin (there is no SIGINT for a process without a shared console, and `CTRL_BREAK` is a termination for Node and Bun). After 10 s, or at once when stdin is already closed after a result, `TerminateJobObject`. No SIGTERM step.
- After `claude` exits, what it left in the job is terminated, and the job must report no active process within 5 s before any outcome is recorded. Otherwise the outcome is `native_agent_outcome_unknown` (Stop must not claim a termination it cannot show).
- Restart: unchanged. The adapter does not declare that its execution ends with Pastey (a broker-started process could outlive it), so a restored turn is unknown and re-observed from the registry and transcript, never sent again.

### H4. Results

| Guarantee | Evidence | Level |
|---|---|---|
| Nothing a contained process starts can leave: explicit `CREATE_BREAKAWAY_FROM_JOB` refused (`ERROR_ACCESS_DENIED`); a child of a process in a nested job that allows silent breakaway (as libuv's) stays in Pastey's job | `nothing_started_inside_the_job_can_leave_it` | Physical |
| The hidden console's `conhost.exe` is in the job too | `a_contained_tree_starts_inside_the_job_and_ends_with_its_termination` (prints the tree) | Physical |
| Owner killed with `TerminateProcess`: `cmd`, two `ping`, `conhost` ended about 1 ms later | `when_the_owner_is_killed_windows_ends_everything_it_contained` | Physical |
| A probe runs inside its own job, without a console window | `a_probe_runs_without_a_console_window_inside_its_own_job` | Physical |
| New session under Pastey's id, exact `--resume`, a new conversation is another session, one session per conversation | `a_new_conversation_…`, `a_follow_up_…` | Physical, fixture |
| Completion only on every piece of evidence; a result is not the end; denials reported; refusal and gone session; a held session refused | 7 tests, as on macOS | Physical, fixture |
| Stop: interrupt through stdin, `claude` ends, its descendant ended with the job, then "cancelled" | `stop_asks_claude_code_itself_then_confirms_every_process_ended` | Physical, fixture |
| Stop ignored: the whole job terminated after the grace (1.5 s in tests, 10 s in the product) | `a_run_that_ignores_stop_is_ended_with_its_whole_job` | Physical, fixture |
| A finished run's leftover process ended before the turn counts | `processes_a_finished_run_left_running_are_ended_before_it_counts` | Physical, fixture |
| Pastey killed (`TerminateProcess`, separate process): the fixture's PowerShell and its `ping` end within 3 s; the restored task is unknown, then `interrupted` from the transcript; never sent again | `killing_pastey_ends_the_run_and_all_it_started_and_a_restart_never_reruns_it` | Physical, fixture |
| Restart during a run: unknown, folder held, observed to Completed | `a_restart_during_a_run_…`, `a_turn_that_ends_while_the_registry_is_asked_…` | Physical, fixture |
| One folder under every name: case (upper, lower), junction, junction + subfolder, verbatim `\\?\`, 8.3 short name when the volume has one, a nested folder and the parent; a sibling folder and another Host run at once | `a_running_folder_is_held_under_every_name_and_independent_folders_run_at_once` | Physical, fixture |
| Trust: untrusted `.claude/settings.json`, `.claude/settings.local.json`, `.mcp.json`, `.claude/skills` refused before anything runs; trusted as Claude Code keys it (`C:/…`), backslashes, lowercase, verbatim, through a junction; not by another folder, the parent outside a repository or a relative key; a repository's trust covers its subfolder | 3 trust tests | Physical, fixture |
| Windows gate: unavailable whatever is installed; an older version than accepted is incompatible; 2.1.309 is older than 2.1.317; signed out | `windows_stays_unavailable_until_its_acceptance_passes`, `availability_…` | Physical, fixture |
| `--bg` continuation unavailable on Windows; no Bridge peer sees or invokes the conversation adapter | 2 tests | Physical, fixture |
| Real model turn, real Bash tree under Stop and under a killed Pastey, interrupt of a running turn | `installed_claude_code_conversation_*` (ignored) | Physical, signed in: see H7 |
| The app's UI (W-CC4), and a paired Mac's view of the capability | the acceptance's in-app steps | **Not run** |

**Mutation checks** (each reverted): without the leftover termination, the leftover test fails; without `KILL_ON_JOB_CLOSE`, both killed-owner tests fail (and the fixture's processes visibly outlive the killed process); without canonical trust matching, the trust test fails; without the H5 fix, its test fails 3 of 3 runs.

Runs: the two Windows suites (`claude_code::job::` and `conversation::windows_tests`) gave 27 passed, 0 failed, 7 ignored in each of 5 consecutive runs.

### H5. A re-observation race (all platforms)

The re-observer read the transcript, then asked the registry. A turn that ended between the two was seen with no process and a stale transcript, and recorded `interrupted` though it had completed. Windows widened the window (a registry read takes about 0.5 s with the real CLI), so a fixture test failed about half the time. The transcript is now read again once the registry shows no process. `a_turn_that_ends_while_the_registry_is_asked_is_observed_as_it_ended` reproduces the interleaving deterministically (the fixture's registry read itself finishes the turn). This is in the shared `ClaudeSessionRunV1::observe`, so it also corrects `--bg` continuation and restored conversations on macOS; the macOS suites were not run here.

### H6. Remaining risks

- **A paired Mac's view of the capability (W-CC4 step 8) is not run** (H8); the app's own steps 1–7 are.
- **Broker-started processes** (WMI, Task Scheduler, services, COM servers) are outside any job. The same class of escape exists on macOS (a process leaving its group).
- **Mapped network drives in trust keys** are resolved like local paths; an unreachable share can delay a start. UNC keys count only as spelled.
- **macOS was not built or tested here** (no cross toolchain on this Host). The Unix code paths change only by `cfg` structure and the H5 fix; the trust matching on Unix now also accepts a key that resolves to the same folder (for example `/tmp/…` for `/private/tmp/…`).
- **2.1.319 is the only signed-in version measured on Windows.** The gate opens only for the version the full acceptance measures.

### H7. Signed-in results, Claude Code 2.1.319 (2026-11-09)

Same Host, the user's own standalone Claude Code (`%USERPROFILE%\.local\bin\claude.exe`, 2.1.319), signed in with claude.ai (Pro). Run from a shell with every Claude Code session variable removed, past the Windows gate by the tests' stand-in (`accept_windows_for_tests`); the product's gate stayed closed. Prompts were harmless, in disposable folders under `src-tauri\target\`. A process monitor recorded every `claude`, `bash`, `ping`, `node` and `conhost` process for the whole run.

- **W-CC1:** the deterministic suites, 27 passed.
- **W-CC2** (`installed_claude_code_conversation_ -- --ignored --test-threads=1`): **3 passed**, twice as a set and once each alone.
  - **Conversation:** availability `Available` ("Claude Code 2.1.319 at …"). A new session completed with its reply; the follow-up resumed exactly it and recalled the reply; both turns name one session; no `claude` process was left.
  - **Stop with a real tool:** Claude Code's Bash tool ran through Git Bash (`claude.exe` → `bash.exe` → `ping.exe -n 90`). Stop recorded `native_agent_cancelled` **1.8 s** after it was asked, so through Claude Code's own interrupt, well inside the 10 s before the job would have been terminated. `claude`, its Bash and the `ping` had all ended. Whether the `ping` was ended by Claude Code's interrupt or by Pastey ending what was left in the job was not distinguished; either way nothing remained before "cancelled" was recorded.
  - **Pastey killed with `TerminateProcess` while the tool ran:** `claude` and the `ping` were gone **17–20 ms** later (three runs), with nobody stopping them. The restored task was `native_agent_outcome_unknown`, then observed `interrupted` (`native_agent_interrupted`) from the transcript, never failed or completed; the run's session holds the prompt once.
- **W-CC3:** after every run, no `ping -n 90` and no `claude -p` process existed; the monitor recorded every process of the runs as ended.
- **First attempt, corrected in the tests only:** headless Claude Code denied the Bash call (it would have asked for approval), so the turn completed before Stop and the Stop test failed as it should. The tool tests now allow `Bash(ping:*)` in their disposable folder's own `.claude\settings.local.json`, and wait for the real `ping` before stopping or killing. Separately, the crash test counted the prompt across earlier runs' sessions of the folder, which Claude Code keeps; it now counts only this run's.
- **Not run here:** W-CC4; see H8.

### H8. The app on Windows (W-CC4, 2026-11-09)

The real app (`target\debug\pastey.exe`, this branch, with `WINDOWS_ACCEPTED_VERSION` set to `(2, 1, 319)` for the run only and restored to `None` afterwards), on throwaway data (`PASTEY_APP_DATA_DIR`), with the same signed-in Claude Code 2.1.319. Desktop control was not available to the session, so the page was driven through WebView2's DevTools port: real mouse and key events in the app's own page, through its real IPC. Two substitutions, both outside what is under test: folders were added through the renderer's own stored folder list instead of Windows' folder dialog (which the DevTools port cannot answer), and the backend's folder rule in step 7 was exercised by calling the app's own `start_native_claude_code_task` command, because the renderer refuses a second task in the same Bridge before it reaches the backend. A watcher logged every new visible top-level window every 25 ms; a process monitor logged every `claude`, `bash`, `ping` and `conhost`.

| # | Result |
|---|---|
| 1 | Settings → Agents: "Claude Code is ready … This device only. Available"; the Bridge's Agents tab: "Codex ready · Claude Code ready". **Pass.** |
| 2 | New conversation → Claude Code → "Reply with the word ok": Completed, "ok"; Task details: `agent.coding.claude_code_conversation`, completed, Conversation new. No console window appeared at any point of the run (0 console-class windows across 7 `claude -p` runs and every probe). **Pass.** |
| 3 | "What did you reply last time?": Completed, "I replied with "ok"."; Conversation continued; Claude Code holds both prompts in one session. **Pass.** |
| 4 | A Bash `ping -n 120` turn, Stop after ~10 s: the `ping` ended within 0.7 s and `claude` within 2.6 s; the task was recorded `native_agent_cancelled` only after that; Claude Code's transcript records its own interrupt. **Pass**, with the renderer finding below. |
| 5 | Same turn, Pastey killed with `taskkill /F` (as Task Manager's End task): `claude` and the `ping` were gone by the monitor's next poll, under 0.42 s. After relaunch the task was `interrupted` / `native_agent_interrupted` (the "outcome unknown" phase had already been resolved from the transcript by the first look), never failed or stopped; the folder list shows the session "Interrupted (inferred)"; the prompt is in its session once. **Pass.** |
| 6 | A never-trusted folder with `.claude\settings.json`: "Did not start · nothing ran", naming `.claude/settings.json`; no `claude -p` started and Claude Code has no record of the folder. The trust-acceptance half was not driven (it needs Claude Code's interactive trust prompt); the `trusted` folder's own `.claude\settings.local.json` ran in steps 2–5 only through Claude Code's `C:/Users/admin/Pastey` trust record. **Pass (refusal); acceptance half covered by the repository's recorded trust.** |
| 7 | While a turn held `trusted`: the junction alias, the upper-case spelling, the plain and verbatim (`\\?\`) forms and the parent folder were all refused before anything started ("already has an unresolved invocation for this resource"); the sibling folder started at once and completed. Once free, the junction ran and was recorded under the canonical folder. **Pass** (backend through the app's command; the UI shows "Claude Code is working on another task in this Bridge" for any second task). |
| 8 | A paired Mac's view (Device details → Agents): **not run**, no Mac. Covered only by `no_bridge_peer_learns_of_it_or_can_invoke_it` and Core's refusal of a Bridge invocation of a capability without peer protocols. |

**Renderer finding (all platforms, not Windows-specific).** After Stop, the task's headline reads "Stopped" while its code is still `native_agent_cancel_requested`, that is, before the native run is confirmed to have ended (here `claude` ran 2.3 s longer). `nativeTaskOutcome.ts` words every `cancelled` state "Stopped" except `native_agent_full_access_revoked` and `native_agent_cancel_delivery_uncertain`. The backend's record is right; the wording claims a stop before it is shown.
