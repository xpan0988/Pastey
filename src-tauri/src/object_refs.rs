//! Receiver-owned, Bridge-scoped temporary object identities.
//!
//! Public descriptors deliberately omit every resolver, path, consent, lease,
//! worker, implementation, and sandbox field. Resolution is a host check, not
//! an authorization decision.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub(crate) const OBJECT_REF_SCHEMA: &str = "pastey-object-ref-v1";
const OBJECT_REF_PREFIX: &str = "object-ref-";
const MAX_OBJECT_REF_LEN: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObjectKind {
    FilesystemCandidate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ObjectRefDescriptor {
    pub(crate) schema_version: String,
    pub(crate) object_ref: String,
    pub(crate) object_kind: ObjectKind,
    pub(crate) owner_device_ref: String,
    pub(crate) bridge_session_ref: String,
    pub(crate) media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
}

#[derive(Clone, Debug)]
pub(crate) enum EphemeralObjectEntry {
    FilesystemCandidate,
}

#[derive(Clone, Debug)]
struct StoredObject {
    descriptor: ObjectRefDescriptor,
    entry: EphemeralObjectEntry,
}

#[derive(Default)]
pub(crate) struct EphemeralObjectStore {
    entries: HashMap<String, StoredObject>,
}

impl EphemeralObjectStore {
    pub(crate) fn register_filesystem_candidate(
        &mut self,
        object_ref: String,
        bridge_session_ref: String,
        owner_device_ref: String,
        media_type: String,
        size_bytes: u64,
        display_name: String,
        created_at: String,
        expires_at: String,
    ) -> AppResult<ObjectRefDescriptor> {
        let descriptor = ObjectRefDescriptor {
            schema_version: OBJECT_REF_SCHEMA.into(),
            object_ref,
            object_kind: ObjectKind::FilesystemCandidate,
            owner_device_ref,
            bridge_session_ref,
            media_type,
            size_bytes: Some(size_bytes),
            display_name: Some(display_name),
            created_at,
            expires_at,
        };
        validate_descriptor(&descriptor, OffsetDateTime::now_utc())?;
        if self.entries.contains_key(&descriptor.object_ref) {
            return Err(AppError::InvalidInput(
                "Filesystem candidate ObjectRef identity is ambiguous.".into(),
            ));
        }
        self.entries.insert(
            descriptor.object_ref.clone(),
            StoredObject {
                descriptor: descriptor.clone(),
                entry: EphemeralObjectEntry::FilesystemCandidate,
            },
        );
        Ok(descriptor)
    }

    pub(crate) fn purge_object(&mut self, object_ref: &str) -> AppResult<bool> {
        let Some(stored) = self.entries.get(object_ref).cloned() else {
            return Ok(false);
        };
        let _ = stored.entry;
        self.entries.remove(object_ref);
        Ok(true)
    }

    pub(crate) fn purge_bridge(&mut self, bridge_session_ref: &str) -> AppResult<usize> {
        let refs = self
            .entries
            .iter()
            .filter_map(|(object_ref, stored)| {
                (stored.descriptor.bridge_session_ref == bridge_session_ref)
                    .then(|| object_ref.clone())
            })
            .collect::<Vec<_>>();
        for object_ref in &refs {
            self.purge_object(object_ref)?;
        }
        Ok(refs.len())
    }

    pub(crate) fn purge_all(&mut self) -> AppResult<usize> {
        let refs = self.entries.keys().cloned().collect::<Vec<_>>();
        for object_ref in &refs {
            self.purge_object(object_ref)?;
        }
        Ok(refs.len())
    }
}

pub(crate) fn new_object_ref() -> String {
    format!("{OBJECT_REF_PREFIX}{}", Uuid::new_v4())
}

pub(crate) fn validate_object_ref(value: &str) -> AppResult<()> {
    if value.len() > MAX_OBJECT_REF_LEN
        || value
            .strip_prefix(OBJECT_REF_PREFIX)
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
        || value.contains('/')
        || value.contains('\\')
        || value.to_ascii_lowercase().starts_with("file:")
    {
        return Err(AppError::InvalidInput("ObjectRef must be opaque.".into()));
    }
    Ok(())
}

pub(crate) fn validate_descriptor(
    descriptor: &ObjectRefDescriptor,
    now: OffsetDateTime,
) -> AppResult<()> {
    validate_object_ref(&descriptor.object_ref)?;
    if descriptor.schema_version != OBJECT_REF_SCHEMA
        || descriptor.owner_device_ref.trim().is_empty()
        || descriptor.owner_device_ref.len() > 256
        || descriptor.bridge_session_ref.trim().is_empty()
        || descriptor.bridge_session_ref.len() > 256
        || descriptor.media_type.trim().is_empty()
        || descriptor.media_type.len() > 128
        || !descriptor.media_type.contains('/')
        || descriptor.display_name.as_deref().is_some_and(|name| {
            name.is_empty() || name.len() > 256 || name.contains('/') || name.contains('\\')
        })
    {
        return Err(AppError::InvalidInput(
            "Invalid ObjectRef descriptor.".into(),
        ));
    }
    let created = parse_time(&descriptor.created_at)?;
    let expires = parse_time(&descriptor.expires_at)?;
    if expires <= created || expires <= now {
        return Err(AppError::InvalidInput(
            "ObjectRef descriptor expired.".into(),
        ));
    }
    Ok(())
}

fn parse_time(value: &str) -> AppResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::InvalidInput("Invalid ObjectRef time.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_refs_are_opaque_and_path_like_values_fail() {
        assert!(validate_object_ref(&new_object_ref()).is_ok());
        for value in [
            "/tmp/file",
            "..\\secret",
            "file:///tmp/file",
            "object-ref-not-a-uuid",
        ] {
            assert!(validate_object_ref(value).is_err());
        }
    }
}
