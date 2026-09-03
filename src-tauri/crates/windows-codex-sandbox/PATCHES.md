# Integration divergence from upstream

Baseline: `openai/codex@ddf8a67ab09cd76b8adc0969f11ee1271179aba7`.

Only the following local divergences are intentional:

1. **Standalone manifests.** The retained sandbox and utility crates use
   explicit crates.io versions and local path dependencies instead of Codex
   workspace inheritance. Bazel metadata is retained for recognizable source
   layout but is not used by Pastey.
2. **Mechanics-only public API.** `src/mechanics.rs` accepts command, cwd,
   environment, explicit read/write roots, stdin state, and sandbox home. It
   constructs the private Codex permission/config types and always selects the
   elevated, network-restricted path. `src/protocol_types.rs` is the minimal
   extraction of Codex product permission types required by retained sandbox
   mechanics; it is private to this crate.
3. **Product telemetry removed.** `codex-otel` settings and WFP metric
   emission were removed. WFP installation, logging, failure continuation,
   and the later native network-denial gate are otherwise unchanged.
4. **Host-owned elevation boundary.** A runtime launch never invokes the
   upstream automatic elevation path when setup is absent or stale. It fails
   closed and instructs the operator to run Pastey's explicit Host-owned setup
   command. The upstream non-elevated root/ACL refresh still runs after valid
   setup is present.
5. **Workspace-only derives removed.** The standalone absolute-path utility
   omits Codex workspace-only schema/TypeScript derives and their dependencies.
6. **Windows GNU compatibility casts.** Two pointer casts in
   `codex-utils-pty/src/win/` use representations accepted by the GNU Windows
   target. They do not change process behavior.
7. **Rust edition lint bridge.** The derived sandbox crate temporarily allows
   `unsafe_op_in_unsafe_fn` while compiling the pinned source as Rust 2024.
8. **Structured runner startup failure and bounded credential recovery.** The
   elevated runner reports the failing startup stage and optional Win32 error
   code to its client. Recognized credential/logon failures receive at most one
   credential refresh and retry; the verifier receives only a bounded,
   secret-free classification of the initial, refresh, and retry outcomes.
   Missing or out-of-date Host setup still fails closed and requires Pastey's
   explicit elevated setup operation.

The helper names, account/setup design, ACL and capability mechanics, WFP and
Firewall mechanics, private desktop behavior, command runner, process session,
and upstream Job behavior are otherwise retained from the pinned revision.
