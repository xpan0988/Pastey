//! Static Host-owned Transform transition registry.

use blake3::Hasher;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionLocality {
    ReceiverHost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectTypePattern {
    pub(crate) media_type: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransformImplementationDescriptor {
    pub(crate) implementation_id: &'static str,
    pub(crate) implementation_version: u32,
    pub(crate) accepted_inputs: &'static [ObjectTypePattern],
    pub(crate) produced_outputs: &'static [ObjectTypePattern],
    pub(crate) execution_locality: ExecutionLocality,
    pub(crate) worker_identity: &'static str,
    pub(crate) resource_policy: &'static str,
    pub(crate) deterministic: bool,
}

const TEXT_INPUTS: &[ObjectTypePattern] = &[
    ObjectTypePattern {
        media_type: "text/plain",
    },
    ObjectTypePattern {
        media_type: "text/markdown",
    },
    ObjectTypePattern {
        media_type: "application/json",
    },
    ObjectTypePattern {
        media_type: "text/csv",
    },
];
const TEXT_OUTPUTS: &[ObjectTypePattern] = &[ObjectTypePattern {
    media_type: "text/plain",
}];

pub(crate) const EXTRACT_READABLE_TEXT_V1: TransformImplementationDescriptor =
    TransformImplementationDescriptor {
        implementation_id: "extract_readable_text_v1",
        implementation_version: 1,
        accepted_inputs: TEXT_INPUTS,
        produced_outputs: TEXT_OUTPUTS,
        execution_locality: ExecutionLocality::ReceiverHost,
        worker_identity: "pastey-transform-text-worker-v1",
        resource_policy: "pastey-transform-text-small-v1",
        deterministic: true,
    };

pub(crate) const TRANSFORM_IMPLEMENTATIONS: &[TransformImplementationDescriptor] =
    &[EXTRACT_READABLE_TEXT_V1];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTransformIntent {
    pub(crate) resolved_intent_ref: String,
    pub(crate) implementation_digest: String,
    pub(crate) implementation: TransformImplementationDescriptor,
    pub(crate) display_description: &'static str,
    pub(crate) output_media_type: &'static str,
}

pub(crate) fn resolve_transform_intent(
    intent: &str,
    input_media_type: &str,
) -> Option<ResolvedTransformIntent> {
    let normalized = intent.trim().to_ascii_lowercase();
    if normalized.len() > 160
        || !matches!(
            normalized.as_str(),
            "extract readable text" | "extract text" | "normalize readable text" | "readable text"
        )
    {
        return None;
    }
    let implementation = TRANSFORM_IMPLEMENTATIONS
        .iter()
        .copied()
        .find(|candidate| {
            candidate
                .accepted_inputs
                .iter()
                .any(|pattern| pattern.media_type == input_media_type)
        })?;
    let implementation_digest = implementation_digest(implementation);
    Some(ResolvedTransformIntent {
        resolved_intent_ref: format!("resolved-intent-{}", uuid::Uuid::new_v4()),
        implementation_digest,
        implementation,
        display_description: "Extract bounded readable text",
        output_media_type: "text/plain",
    })
}

pub(crate) fn implementation_digest(implementation: TransformImplementationDescriptor) -> String {
    let mut hasher = Hasher::new();
    hasher.update(implementation.implementation_id.as_bytes());
    hasher.update(&implementation.implementation_version.to_be_bytes());
    hasher.update(implementation.worker_identity.as_bytes());
    hasher.update(implementation.resource_policy.as_bytes());
    for input in implementation.accepted_inputs {
        hasher.update(input.media_type.as_bytes());
    }
    for output in implementation.produced_outputs {
        hasher.update(output.media_type.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_static_text_only_and_deterministic() {
        assert_eq!(TRANSFORM_IMPLEMENTATIONS, &[EXTRACT_READABLE_TEXT_V1]);
        for media in [
            "text/plain",
            "text/markdown",
            "application/json",
            "text/csv",
        ] {
            let first = resolve_transform_intent("extract readable text", media).unwrap();
            let second = resolve_transform_intent("extract readable text", media).unwrap();
            assert_eq!(first.implementation_digest, second.implementation_digest);
            assert_eq!(first.output_media_type, "text/plain");
        }
        for media in [
            "application/pdf",
            "image/png",
            "application/zip",
            "application/octet-stream",
        ] {
            assert!(resolve_transform_intent("extract readable text", media).is_none());
        }
        assert!(resolve_transform_intent("run python", "text/plain").is_none());
    }
}
