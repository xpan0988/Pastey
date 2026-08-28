//! Host-private managed-object acquisition and physical binding.
//!
//! This service validates a Host-local artifact, establishes an opaque logical
//! object revision at the local Host, and retains the physical path only in a
//! process-local resolver. Acquisition is not a Plan primitive, requester
//! approval, Host admission, Transform, or execution authority.

use std::{collections::HashMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    host_identity::HostRef,
    safe_file_identity::{self, SourceIdentity},
    storage::MAX_FILE_SIZE_BYTES,
};

const MANAGED_OBJECT_PREFIX: &str = "managed-object:v1:";
const MANAGED_BINDING_PREFIX: &str = "managed-object-binding:v1:";
const MAX_TEXT: usize = 256;
const MAX_BINDING_LIFETIME_SECONDS: i64 = 60 * 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedObjectAcquisitionKind {
    SearchResult,
    InboxItem,
    DragDrop,
    LocalSelection,
    GeneratedArtifact,
    TransferReceipt,
}

/// Rust-private physical input. The owning adapter/Core service supplies the
/// path and its already selected safe root; neither is serializable.
#[derive(Clone, Debug)]
pub(crate) struct HostArtifactAcquisition {
    pub(crate) kind: ManagedObjectAcquisitionKind,
    pub(crate) source_ref: String,
    pub(crate) bridge_id: Option<String>,
    pub(crate) path: PathBuf,
    pub(crate) scope_root: PathBuf,
    pub(crate) display_name: String,
    pub(crate) media_type: String,
    pub(crate) expires_at: i64,
    pub(crate) app_owned_temporary: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedLogicalObjectRevision {
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
    pub(crate) host_ref: HostRef,
    pub(crate) media_type: String,
    pub(crate) size_bytes: u64,
    pub(crate) display_name: String,
    pub(crate) acquired_from: ManagedObjectAcquisitionKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedObjectSessionBinding {
    pub(crate) binding_ref: String,
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
    pub(crate) host_ref: HostRef,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedObjectAcquisition {
    pub(crate) object: ManagedLogicalObjectRevision,
    pub(crate) binding: ManagedObjectSessionBinding,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedManagedArtifact {
    pub(crate) path: PathBuf,
    pub(crate) scope_root: PathBuf,
    pub(crate) display_name: String,
    pub(crate) media_type: String,
    pub(crate) size_bytes: u64,
    pub(crate) identity: SourceIdentity,
    pub(crate) app_owned_temporary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RevisionClaim {
    host_ref: HostRef,
    content_digest: String,
    size_bytes: u64,
}

#[derive(Clone, Debug)]
struct StoredBinding {
    acquisition: ManagedObjectAcquisition,
    bridge_id: Option<String>,
    artifact: ResolvedManagedArtifact,
}

pub(crate) struct ManagedObjectBindingService {
    local_host_ref: HostRef,
    bindings: HashMap<String, StoredBinding>,
    revisions: HashMap<(String, u64), RevisionClaim>,
}

impl ManagedObjectBindingService {
    pub(crate) fn new(local_host_ref: HostRef) -> Self {
        Self {
            local_host_ref,
            bindings: HashMap::new(),
            revisions: HashMap::new(),
        }
    }

    /// Establishes revision 1 of a newly acquired logical object.
    #[allow(dead_code)] // Generic Inbox/drag/drop/generated callers attach incrementally.
    pub(crate) fn acquire_new(
        &mut self,
        input: HostArtifactAcquisition,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        if input.kind == ManagedObjectAcquisitionKind::TransferReceipt {
            return Err(AppError::InvalidInput(
                "A Transfer receipt must bind an existing logical revision.".into(),
            ));
        }
        let logical_object_id = format!("{MANAGED_OBJECT_PREFIX}{}", Uuid::new_v4());
        self.acquire(input, logical_object_id, 1, None, now)
    }

    /// Core-owned binding of one authored native-v2 Search output. The logical
    /// identity and revision come from the immutable Plan; candidate/model
    /// data cannot choose either value. This is acquisition, not Transform
    /// lineage, and therefore accepts only SearchResult revision 1.
    pub(crate) fn bind_authored_search_revision(
        &mut self,
        input: HostArtifactAcquisition,
        logical_object_id: String,
        revision: u64,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        if input.kind != ManagedObjectAcquisitionKind::SearchResult || revision != 1 {
            return Err(AppError::InvalidInput(
                "Only an authored Search result may establish revision 1.".into(),
            ));
        }
        validate_managed_object_id(&logical_object_id)?;
        self.acquire(input, logical_object_id, revision, None, now)
    }

    /// Re-establishes the same exact logical revision at this Host after an
    /// explicit Transfer. It cannot create N+1 and requires the expected
    /// content identity from Core-owned lineage.
    #[allow(dead_code)] // Used by protocol v2 after the Phase 4 migration.
    pub(crate) fn bind_transferred_revision(
        &mut self,
        input: HostArtifactAcquisition,
        logical_object_id: String,
        revision: u64,
        expected_content_digest: String,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        if input.kind != ManagedObjectAcquisitionKind::TransferReceipt {
            return Err(AppError::InvalidInput(
                "Only an explicit Transfer receipt can relocate a logical revision.".into(),
            ));
        }
        validate_managed_object_id(&logical_object_id)?;
        if revision == 0 || expected_content_digest.is_empty() {
            return Err(AppError::InvalidInput(
                "Transferred logical revision identity is incomplete.".into(),
            ));
        }
        self.acquire(
            input,
            logical_object_id,
            revision,
            Some(expected_content_digest),
            now,
        )
    }

    /// Core-only Transform finalization. This is the sole additive N+1
    /// registration seam; acquisition, Scratch, and Worker proposals cannot
    /// call it through any serialized contract.
    pub(crate) fn register_core_transform_revision(
        &mut self,
        input: HostArtifactAcquisition,
        logical_object_id: String,
        revision: u64,
        expected_content_digest: String,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        if input.kind != ManagedObjectAcquisitionKind::GeneratedArtifact || revision < 2 {
            return Err(AppError::InvalidInput(
                "Core Transform lineage input is invalid.".into(),
            ));
        }
        validate_managed_object_id(&logical_object_id)?;
        if !self
            .revisions
            .contains_key(&(logical_object_id.clone(), revision - 1))
        {
            return Err(AppError::InvalidInput(
                "Core Transform lineage has no exact prior revision on this Host.".into(),
            ));
        }
        if self
            .revisions
            .contains_key(&(logical_object_id.clone(), revision))
        {
            return Err(AppError::InvalidInput(
                "Core Transform lineage revision is already registered.".into(),
            ));
        }
        self.acquire(
            input,
            logical_object_id,
            revision,
            Some(expected_content_digest),
            now,
        )
    }

    /// v1 compatibility facade. The generic identity is scoped to the exact
    /// immutable Plan revision; callers may continue projecting it as
    /// `selected_file` revision 1 without changing the v1 hash or wire.
    pub(crate) fn acquire_legacy_v1_root(
        &mut self,
        input: HostArtifactAcquisition,
        plan_revision_id: &str,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        validate_token(plan_revision_id, "Plan revision")?;
        if input.kind == ManagedObjectAcquisitionKind::TransferReceipt {
            return Err(AppError::InvalidInput(
                "A v1 root acquisition cannot imply Transfer lineage.".into(),
            ));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-managed-object-v1-compatibility-root\0");
        hasher.update(plan_revision_id.as_bytes());
        let logical_object_id = format!("{MANAGED_OBJECT_PREFIX}{}", hasher.finalize().to_hex());
        self.acquire(input, logical_object_id, 1, None, now)
    }

    pub(crate) fn resolve(
        &mut self,
        acquisition: &ManagedObjectAcquisition,
        now: i64,
    ) -> AppResult<ResolvedManagedArtifact> {
        self.prune_expired(now);
        validate_binding_ref(&acquisition.binding.binding_ref)?;
        let stored = self
            .bindings
            .get(&acquisition.binding.binding_ref)
            .ok_or_else(|| {
                AppError::InvalidInput("Managed object binding is unavailable.".into())
            })?;
        if &stored.acquisition != acquisition
            || acquisition.object.host_ref != self.local_host_ref
            || acquisition.binding.host_ref != self.local_host_ref
            || acquisition.binding.expires_at <= now
        {
            return Err(AppError::InvalidInput(
                "Managed object binding is stale or mismatched.".into(),
            ));
        }
        let observed = safe_file_identity::capture_source_identity(
            &stored.artifact.path,
            &stored.artifact.scope_root,
            MAX_FILE_SIZE_BYTES,
        )?;
        if observed != stored.artifact.identity || observed.byte_count != stored.artifact.size_bytes
        {
            return Err(AppError::InvalidInput(
                "Managed object artifact changed after binding.".into(),
            ));
        }
        Ok(stored.artifact.clone())
    }

    /// Coordinator-only lookup of one already acquired exact local revision.
    /// Logical identity is not a bearer token: ambiguity, expiry, Bridge
    /// substitution, or changed physical identity all fail closed.
    pub(crate) fn acquisition_for_revision(
        &mut self,
        bridge_id: &str,
        logical_object_id: &str,
        revision: u64,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        self.prune_expired(now);
        let matches = self
            .bindings
            .values()
            .filter(|stored| {
                stored.bridge_id.as_deref() == Some(bridge_id)
                    && stored.acquisition.object.logical_object_id == logical_object_id
                    && stored.acquisition.object.revision == revision
                    && stored.acquisition.binding.expires_at > now
            })
            .map(|stored| stored.acquisition.clone())
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(AppError::InvalidInput(
                "Exact managed object revision binding is unavailable or ambiguous.".into(),
            ));
        }
        let acquisition = matches.into_iter().next().expect("one exact binding");
        self.resolve(&acquisition, now)?;
        Ok(acquisition)
    }

    pub(crate) fn purge_bridge(&mut self, bridge_id: &str) -> usize {
        let before = self.bindings.len();
        self.bindings
            .retain(|_, stored| stored.bridge_id.as_deref() != Some(bridge_id));
        self.retain_claimed_revisions();
        before - self.bindings.len()
    }

    pub(crate) fn purge_all(&mut self) -> usize {
        let count = self.bindings.len();
        self.bindings.clear();
        self.revisions.clear();
        count
    }

    fn acquire(
        &mut self,
        input: HostArtifactAcquisition,
        logical_object_id: String,
        revision: u64,
        expected_content_digest: Option<String>,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        self.prune_expired(now);
        validate_input(&input, now)?;
        validate_managed_object_id(&logical_object_id)?;
        if revision == 0 {
            return Err(AppError::InvalidInput(
                "Managed logical revision must be positive.".into(),
            ));
        }
        let scope_root = input
            .scope_root
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Managed object scope is unavailable.".into()))?;
        let path = input.path.canonicalize().map_err(|_| {
            AppError::InvalidInput("Managed object artifact is unavailable.".into())
        })?;
        if !path.starts_with(&scope_root) {
            return Err(AppError::InvalidInput(
                "Managed object artifact escaped its Host-local scope.".into(),
            ));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::InvalidInput(
                "Managed object artifact must be a regular file.".into(),
            ));
        }
        let identity =
            safe_file_identity::capture_source_identity(&path, &scope_root, MAX_FILE_SIZE_BYTES)?;
        if let Some(expected) = expected_content_digest {
            if identity.digest != expected {
                return Err(AppError::InvalidInput(
                    "Transferred artifact does not match the logical revision.".into(),
                ));
            }
        }
        let claim = RevisionClaim {
            host_ref: self.local_host_ref.clone(),
            content_digest: identity.digest.clone(),
            size_bytes: identity.byte_count,
        };
        let key = (logical_object_id.clone(), revision);
        if self
            .revisions
            .get(&key)
            .is_some_and(|existing| existing != &claim)
        {
            return Err(AppError::InvalidInput(
                "Managed logical revision has a conflicting Host artifact.".into(),
            ));
        }

        let object = ManagedLogicalObjectRevision {
            logical_object_id: logical_object_id.clone(),
            revision,
            host_ref: self.local_host_ref.clone(),
            media_type: input.media_type.clone(),
            size_bytes: identity.byte_count,
            display_name: input.display_name.clone(),
            acquired_from: input.kind,
        };
        let binding = ManagedObjectSessionBinding {
            binding_ref: format!("{MANAGED_BINDING_PREFIX}{}", Uuid::new_v4()),
            logical_object_id,
            revision,
            host_ref: self.local_host_ref.clone(),
            expires_at: input.expires_at,
        };
        let acquisition = ManagedObjectAcquisition { object, binding };
        let artifact = ResolvedManagedArtifact {
            path,
            scope_root,
            display_name: input.display_name,
            media_type: input.media_type,
            size_bytes: identity.byte_count,
            identity,
            app_owned_temporary: input.app_owned_temporary,
        };
        self.revisions.insert(key, claim);
        self.bindings.insert(
            acquisition.binding.binding_ref.clone(),
            StoredBinding {
                acquisition: acquisition.clone(),
                bridge_id: input.bridge_id,
                artifact,
            },
        );
        Ok(acquisition)
    }

    fn prune_expired(&mut self, now: i64) {
        self.bindings
            .retain(|_, stored| stored.acquisition.binding.expires_at > now);
        self.retain_claimed_revisions();
    }

    fn retain_claimed_revisions(&mut self) {
        let live = self
            .bindings
            .values()
            .map(|stored| {
                (
                    stored.acquisition.object.logical_object_id.clone(),
                    stored.acquisition.object.revision,
                )
            })
            .collect::<std::collections::HashSet<_>>();
        self.revisions.retain(|key, _| live.contains(key));
    }
}

fn validate_input(input: &HostArtifactAcquisition, now: i64) -> AppResult<()> {
    validate_token(&input.source_ref, "Acquisition source")?;
    if let Some(bridge_id) = &input.bridge_id {
        validate_token(bridge_id, "Bridge")?;
    }
    if input.display_name.is_empty()
        || input.display_name.len() > MAX_TEXT
        || input.display_name.contains('/')
        || input.display_name.contains('\\')
        || input.media_type.is_empty()
        || input.media_type.len() > 128
        || !input.media_type.contains('/')
    {
        return Err(AppError::InvalidInput(
            "Managed object display metadata is invalid.".into(),
        ));
    }
    if input.expires_at <= now || input.expires_at > now + MAX_BINDING_LIFETIME_SECONDS {
        return Err(AppError::InvalidInput(
            "Managed object binding lifetime is invalid.".into(),
        ));
    }
    Ok(())
}

fn validate_token(value: &str, field: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_TEXT
        || value.contains('/')
        || value.contains('\\')
        || value.to_ascii_lowercase().starts_with("file:")
    {
        return Err(AppError::InvalidInput(format!(
            "{field} identity is invalid."
        )));
    }
    Ok(())
}

fn validate_managed_object_id(value: &str) -> AppResult<()> {
    let encoded = value.strip_prefix(MANAGED_OBJECT_PREFIX).ok_or_else(|| {
        AppError::InvalidInput("Invalid managed logical object contract version.".into())
    })?;
    if Uuid::parse_str(encoded).is_err()
        && (encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AppError::InvalidInput(
            "Invalid managed logical object identity.".into(),
        ));
    }
    Ok(())
}

fn validate_binding_ref(value: &str) -> AppResult<()> {
    let encoded = value.strip_prefix(MANAGED_BINDING_PREFIX).ok_or_else(|| {
        AppError::InvalidInput("Invalid managed object binding contract version.".into())
    })?;
    if Uuid::parse_str(encoded).is_err() {
        return Err(AppError::InvalidInput(
            "Invalid managed object binding identity.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(value: &str) -> HostRef {
        HostRef::from_device_id(value).unwrap()
    }

    fn artifact(
        label: &str,
        kind: ManagedObjectAcquisitionKind,
        bridge_id: Option<&str>,
        now: i64,
    ) -> (PathBuf, HostArtifactAcquisition) {
        let root = std::env::temp_dir().join(format!("{label}-{}", Uuid::new_v4()));
        let scope = root.join("scope");
        fs::create_dir_all(&scope).unwrap();
        let path = scope.join("example.txt");
        fs::write(&path, b"managed object contents").unwrap();
        (
            root,
            HostArtifactAcquisition {
                kind,
                source_ref: format!("source-{}", Uuid::new_v4()),
                bridge_id: bridge_id.map(str::to_string),
                path,
                scope_root: scope,
                display_name: "example.txt".into(),
                media_type: "text/plain".into(),
                expires_at: now + 600,
                app_owned_temporary: false,
            },
        )
    }

    #[test]
    fn generic_acquisition_paths_share_one_private_binding_contract() {
        let now = crate::storage::now_ts();
        for kind in [
            ManagedObjectAcquisitionKind::SearchResult,
            ManagedObjectAcquisitionKind::InboxItem,
            ManagedObjectAcquisitionKind::DragDrop,
            ManagedObjectAcquisitionKind::LocalSelection,
            ManagedObjectAcquisitionKind::GeneratedArtifact,
        ] {
            let (root, input) = artifact("pastey-managed-acquisition", kind, None, now);
            let local = host("local");
            let mut binder = ManagedObjectBindingService::new(local.clone());
            let acquired = binder.acquire_new(input, now).unwrap();
            let resolved = binder.resolve(&acquired, now).unwrap();
            assert_eq!(acquired.object.revision, 1);
            assert_eq!(acquired.object.host_ref, local);
            assert_eq!(acquired.object.acquired_from, kind);
            assert_eq!(resolved.display_name, "example.txt");

            let encoded = serde_json::to_value(&acquired).unwrap();
            let text = encoded.to_string();
            assert!(!text.contains(root.to_string_lossy().as_ref()));
            for forbidden in [
                "path",
                "approval",
                "admission",
                "grant",
                "authority",
                "transform",
                "execute",
            ] {
                assert!(!text.to_ascii_lowercase().contains(forbidden));
            }
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn exact_location_revision_and_content_mismatches_fail_closed() {
        let now = crate::storage::now_ts();
        let local = host("local");
        let logical_object_id = format!("{MANAGED_OBJECT_PREFIX}{}", Uuid::new_v4());
        let (first_root, first) = artifact(
            "pastey-managed-transfer-first",
            ManagedObjectAcquisitionKind::TransferReceipt,
            Some("bridge"),
            now,
        );
        let first_digest = safe_file_identity::capture_source_identity(
            &first.path,
            &first.scope_root,
            MAX_FILE_SIZE_BYTES,
        )
        .unwrap()
        .digest;
        let mut binder = ManagedObjectBindingService::new(local.clone());
        let acquired = binder
            .bind_transferred_revision(first, logical_object_id.clone(), 3, first_digest, now)
            .unwrap();
        assert_eq!(acquired.object.revision, 3);
        assert_eq!(acquired.object.host_ref, local);

        let (second_root, second) = artifact(
            "pastey-managed-transfer-conflict",
            ManagedObjectAcquisitionKind::TransferReceipt,
            Some("bridge"),
            now,
        );
        fs::write(&second.path, b"different bytes").unwrap();
        let second_digest = safe_file_identity::capture_source_identity(
            &second.path,
            &second.scope_root,
            MAX_FILE_SIZE_BYTES,
        )
        .unwrap()
        .digest;
        assert!(binder
            .bind_transferred_revision(second, logical_object_id, 3, second_digest, now,)
            .is_err());

        let (wrong_root, wrong_kind) = artifact(
            "pastey-managed-transfer-wrong-kind",
            ManagedObjectAcquisitionKind::LocalSelection,
            Some("bridge"),
            now,
        );
        assert!(binder
            .bind_transferred_revision(
                wrong_kind,
                format!("{MANAGED_OBJECT_PREFIX}{}", Uuid::new_v4()),
                2,
                "digest".into(),
                now,
            )
            .is_err());
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
        let _ = fs::remove_dir_all(wrong_root);
    }

    #[test]
    fn changed_expired_or_burned_physical_bindings_are_unavailable() {
        let now = crate::storage::now_ts();
        let (root, input) = artifact(
            "pastey-managed-binding-lifecycle",
            ManagedObjectAcquisitionKind::SearchResult,
            Some("bridge"),
            now,
        );
        let path = input.path.clone();
        let mut binder = ManagedObjectBindingService::new(host("local"));
        let acquired = binder.acquire_new(input, now).unwrap();
        fs::write(path, b"changed after binding").unwrap();
        assert!(binder.resolve(&acquired, now).is_err());
        assert_eq!(binder.purge_bridge("bridge"), 1);
        assert!(binder.resolve(&acquired, now).is_err());

        let (expired_root, mut expired) = artifact(
            "pastey-managed-binding-expired",
            ManagedObjectAcquisitionKind::InboxItem,
            None,
            now,
        );
        expired.expires_at = now;
        assert!(binder.acquire_new(expired, now).is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(expired_root);
    }

    #[test]
    fn acquisition_rejects_artifacts_outside_the_declared_host_scope() {
        let now = crate::storage::now_ts();
        let (root, mut input) = artifact(
            "pastey-managed-binding-scope",
            ManagedObjectAcquisitionKind::DragDrop,
            None,
            now,
        );
        let outside = root.join("outside.txt");
        fs::write(&outside, b"outside").unwrap();
        input.path = outside;
        assert!(ManagedObjectBindingService::new(host("local"))
            .acquire_new(input, now)
            .is_err());
        let _ = fs::remove_dir_all(root);
    }
}
