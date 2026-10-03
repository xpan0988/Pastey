//! The adapter seam beneath the Native Agent lifecycle.
//!
//! The lifecycle in `native_agent` (task identity, admission, status,
//! cancellation, reconciliation, restart recovery, Bridge authority and the
//! durable envelope) is capability-neutral. A Host capability plugs in here
//! and owns all of its HOW: what its input means, how it runs, how it stops,
//! and what its terminal output contains. Core sees only an opaque payload,
//! an identity digest and an opaque exclusivity key.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{NativeAgentCapabilityStateV1, NativeAgentCapabilityV1, NativeTurnOutcomeV1};
use crate::error::{AppError, AppResult};

pub(super) mod codex;
#[cfg(test)]
pub(super) mod fake_longjob;

/// Bound for one invocation input or terminal output. It keeps either well
/// inside one Room Control event.
pub(crate) const MAX_CAPABILITY_PAYLOAD_BYTES: usize = 16 * 1024;

/// Capability-owned invocation input or terminal output. Core can carry,
/// persist, compare and digest it, but only adapters (this module and its
/// children) can read or build its contents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct OpaqueCapabilityPayloadV1(Value);

impl OpaqueCapabilityPayloadV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if serde_json::to_vec(&self.0)?.len() > MAX_CAPABILITY_PAYLOAD_BYTES {
            return Err(AppError::InvalidInput(
                "Native capability payload exceeds its bound.".into(),
            ));
        }
        Ok(())
    }

    /// Identity of the exact payload. Object keys serialize in sorted order,
    /// so equal JSON values always digest equally.
    pub(crate) fn digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.0).unwrap_or_default();
        blake3::hash(&bytes).to_hex().to_string()
    }

    fn new(value: Value) -> Self {
        Self(value)
    }

    fn value(&self) -> &Value {
        &self.0
    }
}

/// A resource an adapter says at most one unresolved invocation may hold.
/// Core only compares keys; what a key stands for is the adapter's business.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ExclusivityKeyV1(String);

impl ExclusivityKeyV1 {
    fn new(capability_id: &str, adapter_key: &str) -> Self {
        Self(format!("{capability_id}\n{adapter_key}"))
    }
}

/// What Core needs to know about an invocation before admitting it.
pub(crate) struct PreparedInvocationV1 {
    /// Two invocations with the same task identity must have the same digest.
    pub(crate) identity_digest: String,
    pub(crate) exclusivity: Option<ExclusivityKeyV1>,
    /// Renderer-safe label for the task projection.
    pub(crate) label: String,
    /// Host workspace bound to the task. Only workspace-scoped adapters
    /// (Codex) set it; workspace movement and its recovery use it.
    pub(crate) host_workspace: Option<PathBuf>,
}

pub(crate) struct StartedInvocationV1 {
    pub(crate) session_reused: bool,
    pub(crate) run: Box<dyn NativeInvocationRunV1>,
}

/// The adapter's terminal report. Core maps `kind` onto its fixed task
/// states and records `summary` and `output` without reading them.
pub(crate) struct NativeInvocationOutcomeV1 {
    pub(crate) kind: NativeTurnOutcomeV1,
    pub(crate) summary: Option<String>,
    pub(crate) output: Option<OpaqueCapabilityPayloadV1>,
}

/// One started invocation, observed on a Core-owned thread.
pub(crate) trait NativeInvocationRunV1: Send {
    /// Blocks until the capability reports a terminal outcome or Pastey can
    /// no longer observe it (`Unknown`). There is no Pastey deadline.
    fn run(&mut self) -> NativeInvocationOutcomeV1;
    /// After `Unknown`, blocks while the invocation might still be running,
    /// so its exclusivity key stays held.
    fn wait_until_observation_lost(&self);
}

/// A mature Host capability invoked through the Native Agent lifecycle.
pub(crate) trait NativeCapabilityAdapterV1: Send + Sync {
    fn capability_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn availability(&self) -> NativeAgentCapabilityStateV1;
    /// Exact protocols this Host supports for the capability, advertised in
    /// its existing capability fact.
    fn supported_protocols(&self) -> &'static [&'static str];

    fn require_available(&self) -> AppResult<()> {
        match self.availability() {
            NativeAgentCapabilityStateV1::Available => Ok(()),
            NativeAgentCapabilityStateV1::Incompatible => Err(AppError::InvalidInput(
                "Native capability interface is incompatible on this Host.".into(),
            )),
            NativeAgentCapabilityStateV1::Unavailable => Err(AppError::InvalidInput(
                "Native capability is unavailable on this Host.".into(),
            )),
        }
    }

    fn describe(&self) -> NativeAgentCapabilityV1 {
        NativeAgentCapabilityV1 {
            agent_id: self.capability_id().into(),
            display_name: self.display_name().into(),
            state: self.availability(),
        }
    }

    /// Validates the opaque input and reports identity and exclusivity.
    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1>;
    /// Admits and starts one invocation. Called once per task identity.
    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<StartedInvocationV1>;
    /// Best-effort native stop. Pastey has already revoked the invocation's
    /// authority; an error means delivery of the stop is uncertain.
    fn cancel(&self, task_id: &str) -> AppResult<()>;
    /// Burn: stop and forget everything the adapter keeps for the task.
    fn release(&self, task_id: &str);
    fn shutdown(&self);
}
