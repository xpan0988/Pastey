//! Executor-side admission of native capability invocations.
//!
//! Bridge/session authentication proves who is talking. Capability
//! availability proves this Host can run the capability. Neither proves that a
//! peer may make this Host run it. Admission is that third decision. Core
//! makes it, capability-agnostically, before any adapter sees the input.
//!
//! There is no configurable policy, stored grant, ACL or per-peer memory. A
//! remote invocation waits for one process-local, one-shot Host Review.

use serde::Serialize;

use super::OpaqueCapabilityPayloadV1;
use crate::host_identity::HostSessionBinding;

/// How long a remote invocation may wait for Review, also bounded by the
/// Bridge's own expiry.
pub(super) const INVOCATION_REVIEW_TTL_SECONDS: i64 = 2 * 60;
/// Unresolved Reviews one Bridge may hold at once.
pub(super) const MAX_PENDING_REVIEWS_PER_BRIDGE: usize = 8;

pub(super) const REVIEW_REQUIRED_CODE: &str = "native_agent_review_required";
pub(super) const ADMISSION_DENIED_CODE: &str = "native_agent_admission_denied";
pub(super) const REVIEW_EXPIRED_CODE: &str = "native_agent_review_expired";
/// The Review's authority (exact session, Bridge, ownership or capability)
/// ended before Accept could start the invocation.
pub(super) const ADMISSION_REVOKED_CODE: &str = "native_agent_admission_revoked";
/// Accepted, but the capability refused the input or could not start it.
pub(super) const START_REJECTED_CODE: &str = "native_agent_start_rejected";

/// Who asks this Host to run an invocation.
pub(super) enum NativeInvocationPrincipalV1<'a> {
    /// This Host's own user. There is no peer or Bridge.
    Local,
    /// A peer, authenticated as this exact current Bridge session.
    Remote(&'a HostSessionBinding),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeAdmissionDecisionV1 {
    Allow,
    RequireReview,
    /// No fixed rule denies yet; callers already treat it as a non-start.
    #[allow(dead_code)]
    Deny,
}

/// The fixed policy.
pub(super) fn decide(principal: &NativeInvocationPrincipalV1<'_>) -> NativeAdmissionDecisionV1 {
    match principal {
        NativeInvocationPrincipalV1::Local => NativeAdmissionDecisionV1::Allow,
        NativeInvocationPrincipalV1::Remote(_) => NativeAdmissionDecisionV1::RequireReview,
    }
}

/// What Core resumes once an invocation is admitted. Private to Core: a
/// Review looks the same to the renderer whichever path created it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeInvocationContinuationV1 {
    /// `native_agent.invoke`.
    Direct,
    /// An approved workspace movement whose workspace has landed here.
    ReceivedWorkspace { movement_id: String },
}

/// A remote invocation held for Review. It is never persisted and never
/// leaves Core: no adapter sees `input` before Accept, and the renderer never
/// sees it at all.
pub(super) struct PendingNativeInvocationV1 {
    pub(super) task_id: String,
    pub(super) capability_id: String,
    pub(super) binding: HostSessionBinding,
    pub(super) expires_at: i64,
    pub(super) input: OpaqueCapabilityPayloadV1,
    pub(super) continuation: NativeInvocationContinuationV1,
}

/// The renderer-safe facts of one pending Review. It carries no input,
/// path, command, argument, provider or adapter detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeInvocationReviewV1 {
    pub(crate) task_id: String,
    pub(crate) bridge_id: String,
    pub(crate) peer_host_ref: String,
    /// The requesting peer's current session, so the renderer can show the
    /// display label it already has for that Bridge member.
    pub(crate) requesting_peer_session_id: String,
    pub(crate) capability_display_name: String,
    pub(crate) expires_at: i64,
}

impl PendingNativeInvocationV1 {
    pub(super) fn review(&self, capability_display_name: &str) -> NativeInvocationReviewV1 {
        NativeInvocationReviewV1 {
            task_id: self.task_id.clone(),
            bridge_id: self.binding.bridge_id.clone(),
            peer_host_ref: self.binding.peer_host_ref.as_str().to_owned(),
            requesting_peer_session_id: self.binding.peer_route_ref.clone(),
            capability_display_name: capability_display_name.to_owned(),
            expires_at: self.expires_at,
        }
    }
}
