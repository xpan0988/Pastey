//! Receiver-owned, Bridge-scoped temporary object identities.
//!
//! Public descriptors deliberately omit every resolver, path, consent, lease,
//! worker, implementation, and sandbox field. Resolution is a host check, not
//! an authorization decision.

use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub(crate) const OBJECT_REF_SCHEMA: &str = "pastey-object-ref-v1";
const OBJECT_REF_PREFIX: &str = "object-ref-";
const OUTPUT_ROOT_NAME: &str = "transform-objects";
const MAX_OBJECT_REF_LEN: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObjectKind {
    FilesystemCandidate,
    TransformOutput,
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
pub(crate) struct TransformOutputObject {
    pub(crate) private_output_path: PathBuf,
    pub(crate) private_output_root: PathBuf,
    pub(crate) private_digest: String,
}

#[derive(Clone, Debug)]
pub(crate) enum EphemeralObjectEntry {
    FilesystemCandidate,
    TransformOutput(TransformOutputObject),
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

    pub(crate) fn register_transform_output(
        &mut self,
        bridge_session_ref: String,
        owner_device_ref: String,
        private_output_path: PathBuf,
        private_output_root: PathBuf,
        private_digest: String,
        size_bytes: u64,
        display_name: String,
        ttl_seconds: i64,
    ) -> AppResult<ObjectRefDescriptor> {
        validate_private_output(&private_output_path, &private_output_root, size_bytes)?;
        let private_output_path = fs::canonicalize(private_output_path)?;
        let private_output_root = fs::canonicalize(private_output_root)?;
        let created = OffsetDateTime::now_utc();
        let expires = created + time::Duration::seconds(ttl_seconds.clamp(1, 600));
        let descriptor = ObjectRefDescriptor {
            schema_version: OBJECT_REF_SCHEMA.into(),
            object_ref: new_object_ref(),
            object_kind: ObjectKind::TransformOutput,
            owner_device_ref,
            bridge_session_ref,
            media_type: "text/plain".into(),
            size_bytes: Some(size_bytes),
            display_name: Some(display_name),
            created_at: created
                .format(&Rfc3339)
                .map_err(|_| AppError::InvalidInput("Invalid ObjectRef time.".into()))?,
            expires_at: expires
                .format(&Rfc3339)
                .map_err(|_| AppError::InvalidInput("Invalid ObjectRef expiry.".into()))?,
        };
        validate_descriptor(&descriptor, created)?;
        self.entries.insert(
            descriptor.object_ref.clone(),
            StoredObject {
                descriptor: descriptor.clone(),
                entry: EphemeralObjectEntry::TransformOutput(TransformOutputObject {
                    private_output_path,
                    private_output_root,
                    private_digest,
                }),
            },
        );
        Ok(descriptor)
    }

    pub(crate) fn resolve(
        &mut self,
        object_ref: &str,
        bridge_session_ref: &str,
        owner_device_ref: &str,
        expected_kind: ObjectKind,
    ) -> AppResult<(ObjectRefDescriptor, EphemeralObjectEntry)> {
        validate_object_ref(object_ref)?;
        let now = OffsetDateTime::now_utc();
        let expired = self
            .entries
            .get(object_ref)
            .and_then(|stored| parse_time(&stored.descriptor.expires_at).ok())
            .is_some_and(|expiry| expiry <= now);
        if expired {
            self.purge_object(object_ref)?;
            return Err(AppError::InvalidInput("ObjectRef expired.".into()));
        }
        let stored = self
            .entries
            .get(object_ref)
            .ok_or_else(|| AppError::NotFound("ObjectRef not found.".into()))?;
        if stored.descriptor.bridge_session_ref != bridge_session_ref {
            return Err(AppError::InvalidInput(
                "ObjectRef Bridge binding mismatch.".into(),
            ));
        }
        if stored.descriptor.owner_device_ref != owner_device_ref {
            return Err(AppError::InvalidInput(
                "ObjectRef owner binding mismatch.".into(),
            ));
        }
        if stored.descriptor.object_kind != expected_kind {
            return Err(AppError::InvalidInput(
                "ObjectRef kind binding mismatch.".into(),
            ));
        }
        if let EphemeralObjectEntry::TransformOutput(output) = &stored.entry {
            validate_private_output(
                &output.private_output_path,
                &output.private_output_root,
                stored.descriptor.size_bytes.unwrap_or_default(),
            )?;
            let digest = blake3::hash(&fs::read(&output.private_output_path)?)
                .to_hex()
                .to_string();
            if digest != output.private_digest {
                return Err(AppError::InvalidInput(
                    "Transform output identity changed.".into(),
                ));
            }
        }
        Ok((stored.descriptor.clone(), stored.entry.clone()))
    }

    pub(crate) fn purge_object(&mut self, object_ref: &str) -> AppResult<bool> {
        let Some(stored) = self.entries.get(object_ref).cloned() else {
            return Ok(false);
        };
        if let EphemeralObjectEntry::TransformOutput(output) = stored.entry {
            cleanup_private_output(&output)?;
        }
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

pub(crate) fn cleanup_orphaned_transform_objects(app_data_dir: &Path) -> AppResult<usize> {
    let root = app_data_dir.join(OUTPUT_ROOT_NAME);
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Transform object root is invalid.".into(),
        ));
    }
    remove_tree(&root)?;
    Ok(1)
}

pub(crate) fn create_transform_output_root(app_data_dir: &Path) -> AppResult<PathBuf> {
    let app_data = fs::canonicalize(app_data_dir)?;
    let parent = app_data.join(OUTPUT_ROOT_NAME);
    if !parent.exists() {
        fs::create_dir(&parent)?;
    }
    let parent = fs::canonicalize(parent)?;
    if !parent.starts_with(&app_data) {
        return Err(AppError::InvalidInput(
            "Transform object root escaped app data.".into(),
        ));
    }
    let root = parent.join(format!("object-output-{}", Uuid::new_v4()));
    fs::create_dir(&root)?;
    Ok(root)
}

fn validate_private_output(path: &Path, root: &Path, size_bytes: u64) -> AppResult<()> {
    let root = fs::canonicalize(root)
        .map_err(|_| AppError::InvalidInput("Transform output root is unavailable.".into()))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| AppError::InvalidInput("Transform output is unavailable.".into()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != size_bytes {
        return Err(AppError::InvalidInput(
            "Transform output is invalid.".into(),
        ));
    }
    let path = fs::canonicalize(path)
        .map_err(|_| AppError::InvalidInput("Transform output is unavailable.".into()))?;
    if !path.starts_with(&root) {
        return Err(AppError::InvalidInput(
            "Transform output escaped private storage.".into(),
        ));
    }
    Ok(())
}

fn cleanup_private_output(output: &TransformOutputObject) -> AppResult<()> {
    let root = fs::canonicalize(&output.private_output_root).or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(output.private_output_root.clone())
        } else {
            Err(error)
        }
    })?;
    if !root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.strip_prefix("object-output-")
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_some()
        })
    {
        return Err(AppError::InvalidInput(
            "Transform output cleanup root is invalid.".into(),
        ));
    }
    remove_tree(&root)
}

fn remove_tree(path: &Path) -> AppResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(AppError::InvalidInput("Refusing symlink cleanup.".into()));
    }
    if metadata.is_file() {
        fs::remove_file(path)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Refusing special-file cleanup.".into(),
        ));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if Path::new(&entry.file_name())
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput("Invalid cleanup child.".into()));
        }
        remove_tree(&entry.path())?;
    }
    fs::remove_dir(path)?;
    Ok(())
}

fn parse_time(value: &str) -> AppResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::InvalidInput("Invalid ObjectRef time.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times() -> (String, String) {
        let now = OffsetDateTime::now_utc();
        (
            now.format(&Rfc3339).unwrap(),
            (now + time::Duration::minutes(1)).format(&Rfc3339).unwrap(),
        )
    }

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

    #[test]
    fn resolution_is_exact_and_burn_purges_bridge_entries() {
        let mut store = EphemeralObjectStore::default();
        let object_ref = new_object_ref();
        let (created, expires) = times();
        store
            .register_filesystem_candidate(
                object_ref.clone(),
                "bridge".into(),
                "owner".into(),
                "text/plain".into(),
                1,
                "note.txt".into(),
                created,
                expires,
            )
            .unwrap();
        assert!(store
            .resolve(
                &object_ref,
                "bridge",
                "owner",
                ObjectKind::FilesystemCandidate
            )
            .is_ok());
        assert!(store
            .resolve(
                &object_ref,
                "wrong",
                "owner",
                ObjectKind::FilesystemCandidate
            )
            .is_err());
        assert!(store
            .resolve(
                &object_ref,
                "bridge",
                "wrong",
                ObjectKind::FilesystemCandidate
            )
            .is_err());
        assert!(store
            .resolve(&object_ref, "bridge", "owner", ObjectKind::TransformOutput)
            .is_err());
        assert_eq!(store.purge_bridge("bridge").unwrap(), 1);
        assert!(store
            .resolve(
                &object_ref,
                "bridge",
                "owner",
                ObjectKind::FilesystemCandidate
            )
            .is_err());
    }

    #[test]
    fn expired_refs_are_removed() {
        let mut store = EphemeralObjectStore::default();
        let object_ref = new_object_ref();
        let created = (OffsetDateTime::now_utc() - time::Duration::minutes(2))
            .format(&Rfc3339)
            .unwrap();
        let expires = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
            .format(&Rfc3339)
            .unwrap();
        let descriptor = ObjectRefDescriptor {
            schema_version: OBJECT_REF_SCHEMA.into(),
            object_ref: object_ref.clone(),
            object_kind: ObjectKind::FilesystemCandidate,
            owner_device_ref: "owner".into(),
            bridge_session_ref: "bridge".into(),
            media_type: "text/plain".into(),
            size_bytes: Some(1),
            display_name: Some("a.txt".into()),
            created_at: created,
            expires_at: expires,
        };
        store.entries.insert(
            object_ref.clone(),
            StoredObject {
                descriptor,
                entry: EphemeralObjectEntry::FilesystemCandidate,
            },
        );
        assert!(store
            .resolve(
                &object_ref,
                "bridge",
                "owner",
                ObjectKind::FilesystemCandidate
            )
            .is_err());
        assert!(!store.entries.contains_key(&object_ref));
    }
}
