//! Host-private Phase 5 managed resource resolution.
//!
//! Opaque resource handles remain the authority roots. Physical paths exist
//! only inside this process-local service and are never serialized. The
//! service supports managed-revision reads and private workspace/output/scratch
//! storage; it cannot spawn processes, access a network, or register lineage.

#![allow(dead_code)] // Live v2 managed execution remains intentionally disabled.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    effect_authority::{
        AuthorityContextRefV1, AuthorityContextV1, BackendApplyV1, BackendEffectOutcomeV1,
        CurrentHostAuthorityV1, EffectAuthorityStateV1, EffectDecisionV1, EffectEnvelopeRefV1,
        EffectEvidenceV1, EffectFactsV1, EffectPreconditionV1, EffectRequestIdV1,
        EffectRequestKindV1, EffectRequestV1, HostEffectBackendV1, ManagedRunRefV1,
        ResourceGrantV1, ResourceHandleRefV1, ResourceKindV1, ResourceVerbV1,
    },
    error::{AppError, AppResult},
    managed_objects::{
        ManagedObjectAcquisition, ManagedObjectBindingService, ResolvedManagedArtifact,
    },
    safe_file_identity::{self, SourceIdentity},
};

const RESOURCE_RESOLUTION_VERSION: &str = "pastey-managed-resource-resolution-v1";
const MAX_SELECTOR_BYTES: usize = 512;

#[derive(Clone, Debug)]
pub(crate) struct ManagedResourceAccessV1 {
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context: AuthorityContextV1,
    pub(crate) current: CurrentHostAuthorityV1,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceBindingSpecV1 {
    pub(crate) acquisition: ManagedObjectAcquisition,
    pub(crate) initial_selector: String,
    pub(crate) quota_bytes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ExecutableBindingSpecV1 {
    pub(crate) executable_path: PathBuf,
    pub(crate) scope_root: PathBuf,
}

/// Host-private mount material consumed only by a verified ExecutionWorld
/// adapter. Neither path is serialized or returned to a Worker.
#[derive(Clone, Debug)]
pub(crate) struct ExecutionWorldMountV1 {
    pub(crate) handle_ref: ResourceHandleRefV1,
    pub(crate) kind: ResourceKindV1,
    pub(crate) source_path: PathBuf,
    pub(crate) mount_name: String,
    pub(crate) writable: bool,
    pub(crate) quota_bytes: u64,
    pub(crate) allowed_verbs: BTreeSet<ResourceVerbV1>,
    pub(crate) private_overlay: bool,
    pub(crate) initial_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SealedOutputEvidenceV1 {
    pub(crate) contract_version: String,
    pub(crate) seal_ref: String,
    pub(crate) evidence_id: String,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) handle_ref: ResourceHandleRefV1,
    pub(crate) relative_selector: String,
    pub(crate) generation: u64,
    pub(crate) content_digest: String,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostResourceReadV1 {
    pub(crate) request_id: EffectRequestIdV1,
    pub(crate) bytes: Vec<u8>,
    pub(crate) generation: u64,
    pub(crate) content_digest: String,
}

#[derive(Clone, Debug)]
struct ExactOwnerV1 {
    envelope_ref: EffectEnvelopeRefV1,
    run_control_ref: ManagedRunRefV1,
    context_ref: AuthorityContextRefV1,
    bridge_id: String,
}

#[derive(Clone, Debug)]
struct PrivateFileV1 {
    path: PathBuf,
    identity: SourceIdentity,
    generation: u64,
    sealed: bool,
    lineage_registered: bool,
    last_request_id: Option<EffectRequestIdV1>,
}

#[derive(Clone, Debug)]
enum HostResourceBackingV1 {
    ManagedRevision {
        owner: ExactOwnerV1,
        acquisition: ManagedObjectAcquisition,
        maximum_bytes: u64,
    },
    Workspace {
        owner: ExactOwnerV1,
        acquisition: ManagedObjectAcquisition,
        root: PathBuf,
        files: HashMap<String, PrivateFileV1>,
        quota_bytes: u64,
    },
    OutputSlot {
        owner: ExactOwnerV1,
        root: PathBuf,
        files: HashMap<String, PrivateFileV1>,
        quota_bytes: u64,
    },
    Scratch {
        owner: ExactOwnerV1,
        root: PathBuf,
        files: HashMap<String, PrivateFileV1>,
        quota_bytes: u64,
    },
    Executable {
        owner: ExactOwnerV1,
        path: PathBuf,
        scope_root: PathBuf,
        identity: SourceIdentity,
    },
}

impl HostResourceBackingV1 {
    fn owner(&self) -> &ExactOwnerV1 {
        match self {
            Self::ManagedRevision { owner, .. }
            | Self::Workspace { owner, .. }
            | Self::OutputSlot { owner, .. }
            | Self::Scratch { owner, .. }
            | Self::Executable { owner, .. } => owner,
        }
    }

    fn kind(&self) -> ResourceKindV1 {
        match self {
            Self::ManagedRevision { .. } => ResourceKindV1::ManagedRevision,
            Self::Workspace { .. } => ResourceKindV1::Workspace,
            Self::OutputSlot { .. } => ResourceKindV1::OutputSlot,
            Self::Scratch { .. } => ResourceKindV1::Scratch,
            Self::Executable { .. } => ResourceKindV1::Executable,
        }
    }

    fn private_root(&self) -> Option<&Path> {
        match self {
            Self::ManagedRevision { .. } => None,
            Self::Workspace { root, .. }
            | Self::OutputSlot { root, .. }
            | Self::Scratch { root, .. } => Some(root),
            Self::Executable { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
struct StagedPayloadV1 {
    owner: ExactOwnerV1,
    handle_ref: ResourceHandleRefV1,
    content_digest: String,
    bytes: Vec<u8>,
}

/// Process-local Host resolver. `base_root` is injected by HostRuntime, but a
/// fresh random process root prevents stale files from recovering authority.
pub(crate) struct ManagedResourceResolverV1 {
    process_root: PathBuf,
    backings: HashMap<ResourceHandleRefV1, HostResourceBackingV1>,
    staged_payloads: HashMap<String, StagedPayloadV1>,
    reads: HashMap<EffectRequestIdV1, HostResourceReadV1>,
    world_leases: HashSet<ResourceHandleRefV1>,
}

impl ManagedResourceResolverV1 {
    pub(crate) fn new(base_root: PathBuf) -> Self {
        Self {
            process_root: base_root.join(format!("run-{}", Uuid::new_v4())),
            backings: HashMap::new(),
            staged_payloads: HashMap::new(),
            reads: HashMap::new(),
            world_leases: HashSet::new(),
        }
    }

    pub(crate) fn managed_revision_identity_ref(
        acquisition: &ManagedObjectAcquisition,
        artifact: &ResolvedManagedArtifact,
    ) -> AppResult<String> {
        identity_ref(
            "pastey-managed-revision-safe-identity-v1",
            acquisition,
            artifact,
        )
    }

    pub(crate) fn workspace_identity_ref(
        acquisition: &ManagedObjectAcquisition,
        artifact: &ResolvedManagedArtifact,
    ) -> AppResult<String> {
        identity_ref("pastey-workspace-safe-identity-v1", acquisition, artifact)
    }

    pub(crate) fn executable_identity_ref(spec: &ExecutableBindingSpecV1) -> AppResult<String> {
        let identity = safe_file_identity::capture_source_identity(
            &spec.executable_path,
            &spec.scope_root,
            u64::MAX,
        )?;
        domain_hash(
            "pastey-executable-safe-identity-v1",
            &(identity.digest, identity.byte_count),
        )
    }

    pub(crate) fn bind_executable(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        spec: ExecutableBindingSpecV1,
    ) -> AppResult<()> {
        let grant = validate_attachment(authority, access, handle_ref, ResourceKindV1::Executable)?;
        let identity = safe_file_identity::capture_source_identity(
            &spec.executable_path,
            &spec.scope_root,
            u64::MAX,
        )?;
        let identity_ref = domain_hash(
            "pastey-executable-safe-identity-v1",
            &(identity.digest.clone(), identity.byte_count),
        )?;
        if grant.safe_identity_ref != identity_ref {
            return invalid("Executable safe identity does not match its resource grant.");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&spec.executable_path)?.permissions().mode() & 0o111 == 0 {
                return invalid("Executable resource is not executable on this Host.");
            }
        }
        self.insert_backing(
            handle_ref,
            HostResourceBackingV1::Executable {
                owner: owner(access)?,
                path: spec.executable_path,
                scope_root: spec.scope_root,
                identity,
            },
        )
    }

    pub(crate) fn bind_managed_revision(
        &mut self,
        authority: &EffectAuthorityStateV1,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        acquisition: ManagedObjectAcquisition,
    ) -> AppResult<()> {
        let grant = validate_attachment(
            authority,
            access,
            handle_ref,
            ResourceKindV1::ManagedRevision,
        )?;
        validate_exact_input(&access.context, &acquisition)?;
        let artifact = objects.resolve(&acquisition, access.current.now)?;
        if grant.safe_identity_ref != Self::managed_revision_identity_ref(&acquisition, &artifact)?
        {
            return invalid("Managed revision safe identity does not match its resource grant.");
        }
        self.insert_backing(
            handle_ref,
            HostResourceBackingV1::ManagedRevision {
                owner: owner(access)?,
                acquisition,
                maximum_bytes: grant.budgets.read_bytes,
            },
        )
    }

    pub(crate) fn bind_workspace(
        &mut self,
        authority: &EffectAuthorityStateV1,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        spec: WorkspaceBindingSpecV1,
    ) -> AppResult<()> {
        let grant = validate_attachment(authority, access, handle_ref, ResourceKindV1::Workspace)?;
        validate_exact_input(&access.context, &spec.acquisition)?;
        validate_managed_resource_selector(&spec.initial_selector)?;
        let artifact = objects.resolve(&spec.acquisition, access.current.now)?;
        if grant.safe_identity_ref != Self::workspace_identity_ref(&spec.acquisition, &artifact)? {
            return invalid("Workspace safe identity does not match its resource grant.");
        }
        validate_quota(
            spec.quota_bytes,
            grant.budgets.write_bytes.max(grant.budgets.read_bytes),
        )?;
        let root = self.allocate_root(handle_ref)?;
        let bytes = safe_file_identity::read_source_if_identity_matches(
            &artifact.path,
            &artifact.scope_root,
            &artifact.identity,
            spec.quota_bytes,
        )?;
        let file = write_private_file(&root, &spec.initial_selector, None, &bytes, 1)?;
        let mut files = HashMap::new();
        files.insert(spec.initial_selector, file);
        self.insert_backing(
            handle_ref,
            HostResourceBackingV1::Workspace {
                owner: owner(access)?,
                acquisition: spec.acquisition,
                root,
                files,
                quota_bytes: spec.quota_bytes,
            },
        )
    }

    pub(crate) fn provision_output_slot(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        quota_bytes: u64,
    ) -> AppResult<()> {
        self.provision_empty(
            authority,
            access,
            handle_ref,
            ResourceKindV1::OutputSlot,
            quota_bytes,
        )
    }

    pub(crate) fn provision_scratch(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        quota_bytes: u64,
    ) -> AppResult<()> {
        self.provision_empty(
            authority,
            access,
            handle_ref,
            ResourceKindV1::Scratch,
            quota_bytes,
        )
    }

    fn provision_empty(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        kind: ResourceKindV1,
        quota_bytes: u64,
    ) -> AppResult<()> {
        let grant = validate_attachment(authority, access, handle_ref, kind)?;
        validate_quota(quota_bytes, grant.budgets.write_bytes)?;
        let root = self.allocate_root(handle_ref)?;
        let backing = match kind {
            ResourceKindV1::OutputSlot => HostResourceBackingV1::OutputSlot {
                owner: owner(access)?,
                root,
                files: HashMap::new(),
                quota_bytes,
            },
            ResourceKindV1::Scratch => HostResourceBackingV1::Scratch {
                owner: owner(access)?,
                root,
                files: HashMap::new(),
                quota_bytes,
            },
            _ => return invalid("Only OutputSlot or Scratch may be provisioned empty."),
        };
        self.insert_backing(handle_ref, backing)
    }

    /// Stages content in Host-private memory. The digest, handle, and exact
    /// owner context are rechecked when the ordered EffectRequest consumes it.
    pub(crate) fn stage_write_payload(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        content_digest: &str,
        bytes: Vec<u8>,
    ) -> AppResult<()> {
        let backing = self.backings.get(handle_ref).ok_or_else(|| {
            AppError::InvalidInput("Managed resource backing is unavailable.".into())
        })?;
        let grant = validate_attachment(authority, access, handle_ref, backing.kind())?;
        validate_owner(backing.owner(), access)?;
        if backing.kind() == ResourceKindV1::ManagedRevision {
            return invalid("Authoritative managed revisions are immutable to Worker authority.");
        }
        let backing_quota = match backing {
            HostResourceBackingV1::Workspace { quota_bytes, .. }
            | HostResourceBackingV1::OutputSlot { quota_bytes, .. }
            | HostResourceBackingV1::Scratch { quota_bytes, .. } => *quota_bytes,
            HostResourceBackingV1::ManagedRevision { .. } => 0,
            HostResourceBackingV1::Executable { .. } => 0,
        };
        if bytes.len() as u64 > grant.budgets.write_bytes
            || bytes.len() as u64 > backing_quota
            || digest_bytes(&bytes) != content_digest
        {
            return invalid("Staged resource payload exceeds authority or has the wrong digest.");
        }
        if self.staged_payloads.values().any(|payload| {
            payload.handle_ref == *handle_ref
                && payload.owner.run_control_ref == access.run_control_ref
        }) {
            return invalid("A managed resource handle already has a pending staged payload.");
        }
        let key = staged_key(&access.run_control_ref, handle_ref, content_digest);
        if self.staged_payloads.contains_key(&key) {
            return invalid("Staged resource payload is duplicated.");
        }
        self.staged_payloads.insert(
            key,
            StagedPayloadV1 {
                owner: owner(access)?,
                handle_ref: handle_ref.clone(),
                content_digest: content_digest.to_owned(),
                bytes,
            },
        );
        Ok(())
    }

    pub(crate) fn take_read(
        &mut self,
        request_id: &EffectRequestIdV1,
    ) -> Option<HostResourceReadV1> {
        self.reads.remove(request_id)
    }

    /// Removes a not-yet-consumed Worker write staging buffer after Host
    /// enforcement denied or reported the effect unavailable. The staged bytes
    /// are not an effect and must not prevent a later, different in-run tool
    /// request from self-correcting.
    pub(crate) fn discard_staged_write_payload(
        &mut self,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        content_digest: &str,
    ) -> AppResult<()> {
        let key = staged_key(&access.run_control_ref, handle_ref, content_digest);
        let Some(staged) = self.staged_payloads.get(&key) else {
            return Ok(());
        };
        if staged.handle_ref != *handle_ref {
            return invalid("Staged resource payload owner was substituted.");
        }
        validate_owner(&staged.owner, access)?;
        self.staged_payloads.remove(&key);
        Ok(())
    }

    pub(crate) fn seal_output_slot(
        &mut self,
        authority: &EffectAuthorityStateV1,
        access: &ManagedResourceAccessV1,
        handle_ref: &ResourceHandleRefV1,
        relative_selector: &str,
        evidence: &EffectEvidenceV1,
    ) -> AppResult<SealedOutputEvidenceV1> {
        validate_attachment(authority, access, handle_ref, ResourceKindV1::OutputSlot)?;
        validate_managed_resource_selector(relative_selector)?;
        authority.validate_terminal_resource_evidence(
            evidence,
            handle_ref,
            &access.envelope_ref,
            &access.run_control_ref,
            &access.context.context_ref()?,
        )?;
        let backing = self
            .backings
            .get_mut(handle_ref)
            .ok_or_else(|| AppError::InvalidInput("Output slot backing is unavailable.".into()))?;
        validate_owner(backing.owner(), access)?;
        let HostResourceBackingV1::OutputSlot { root, files, .. } = backing else {
            return invalid("Only OutputSlot resources may be sealed.");
        };
        let file = files.get_mut(relative_selector).ok_or_else(|| {
            AppError::InvalidInput("Output slot generation is unavailable.".into())
        })?;
        let observed = safe_file_identity::capture_source_identity(
            &file.path,
            root,
            file.identity.byte_count,
        )?;
        let facts_match = matches!(
            &evidence.facts,
            EffectFactsV1::Resource { generation, content_digest, bytes, .. }
                if *generation == file.generation
                    && content_digest == &observed.digest
                    && *bytes == observed.byte_count
        );
        if file.sealed
            || file.last_request_id.as_ref() != Some(&evidence.request_id)
            || observed != file.identity
            || !facts_match
        {
            return invalid("Output slot generation or evidence is stale or mismatched.");
        }
        file.sealed = true;
        let context_ref = access.context.context_ref()?;
        let seal_ref = domain_hash(
            "pastey-output-slot-seal-v1",
            &(
                evidence.evidence_id.as_str(),
                handle_ref.as_str(),
                relative_selector,
                file.generation,
                &observed.digest,
                observed.byte_count,
            ),
        )?;
        Ok(SealedOutputEvidenceV1 {
            contract_version: RESOURCE_RESOLUTION_VERSION.into(),
            seal_ref,
            evidence_id: evidence.evidence_id.as_str().to_owned(),
            envelope_ref: access.envelope_ref.clone(),
            run_control_ref: access.run_control_ref.clone(),
            context_ref,
            handle_ref: handle_ref.clone(),
            relative_selector: relative_selector.to_owned(),
            generation: file.generation,
            content_digest: observed.digest,
            bytes: observed.byte_count,
        })
    }

    /// Core-only promotion of one exact sealed output generation into N+1.
    /// The private path never crosses into a Worker/result proposal.
    pub(crate) fn register_sealed_transform_output(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        seal: &SealedOutputEvidenceV1,
        logical_object_id: String,
        output_revision: u64,
        display_name: String,
        media_type: String,
        expires_at: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        if seal.envelope_ref != access.envelope_ref
            || seal.run_control_ref != access.run_control_ref
            || seal.context_ref != access.context.context_ref()?
        {
            return invalid("Sealed output context was substituted.");
        }
        let backing = self.backings.get_mut(&seal.handle_ref).ok_or_else(|| {
            AppError::InvalidInput("Sealed output backing is unavailable.".into())
        })?;
        validate_owner(backing.owner(), access)?;
        let HostResourceBackingV1::OutputSlot { root, files, .. } = backing else {
            return invalid("Transform lineage requires an OutputSlot.");
        };
        let file = files.get_mut(&seal.relative_selector).ok_or_else(|| {
            AppError::InvalidInput("Sealed output generation is unavailable.".into())
        })?;
        let observed = safe_file_identity::capture_source_identity(
            &file.path,
            root,
            file.identity.byte_count,
        )?;
        if !file.sealed
            || file.lineage_registered
            || observed != file.identity
            || observed.digest != seal.content_digest
            || observed.byte_count != seal.bytes
            || file.generation != seal.generation
        {
            return invalid("Sealed output identity is stale, reused, or mismatched.");
        }
        let acquisition = objects.register_core_transform_revision(
            crate::managed_objects::HostArtifactAcquisition {
                kind: crate::managed_objects::ManagedObjectAcquisitionKind::GeneratedArtifact,
                source_ref: seal.seal_ref.clone(),
                bridge_id: Some(access.context.bridge_id.clone()),
                path: file.path.clone(),
                scope_root: root.clone(),
                display_name,
                media_type,
                expires_at,
                app_owned_temporary: true,
            },
            logical_object_id,
            output_revision,
            seal.content_digest.clone(),
            access.current.now,
        )?;
        file.lineage_registered = true;
        Ok(acquisition)
    }

    /// Resolves exact envelope-owned handles into a one-use Host-private mount
    /// lease. Paths never leave Host code. Safe identities are revalidated
    /// immediately before a platform adapter constructs its confined world.
    pub(crate) fn lease_execution_world_mounts(
        &mut self,
        authority: &EffectAuthorityStateV1,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        grants: &[ResourceGrantV1],
    ) -> AppResult<Vec<ExecutionWorldMountV1>> {
        let requested = grants
            .iter()
            .map(|grant| grant.handle_ref.clone())
            .collect::<BTreeSet<_>>();
        if requested.len() != grants.len()
            || requested
                .iter()
                .any(|handle| self.world_leases.contains(handle))
        {
            return invalid("Execution world resource handle is duplicated or already leased.");
        }
        let mut mounts = Vec::with_capacity(grants.len());
        for grant in grants {
            validate_attachment(authority, access, &grant.handle_ref, grant.kind)?;
            let backing = self.backings.get(&grant.handle_ref).ok_or_else(|| {
                AppError::InvalidInput("Execution world resource backing is unavailable.".into())
            })?;
            validate_owner(backing.owner(), access)?;
            let (source_path, quota_bytes) = match backing {
                HostResourceBackingV1::ManagedRevision {
                    acquisition,
                    maximum_bytes,
                    ..
                } => {
                    let artifact = objects.resolve(acquisition, access.current.now)?;
                    safe_file_identity::read_source_if_identity_matches(
                        &artifact.path,
                        &artifact.scope_root,
                        &artifact.identity,
                        *maximum_bytes,
                    )?;
                    (artifact.path, *maximum_bytes)
                }
                HostResourceBackingV1::Workspace {
                    acquisition,
                    root,
                    files,
                    quota_bytes,
                    ..
                } => {
                    objects.resolve(acquisition, access.current.now)?;
                    validate_private_tree(root, files, *quota_bytes)?;
                    (root.clone(), *quota_bytes)
                }
                HostResourceBackingV1::OutputSlot {
                    root,
                    files,
                    quota_bytes,
                    ..
                }
                | HostResourceBackingV1::Scratch {
                    root,
                    files,
                    quota_bytes,
                    ..
                } => {
                    validate_private_tree(root, files, *quota_bytes)?;
                    (root.clone(), *quota_bytes)
                }
                HostResourceBackingV1::Executable {
                    path,
                    scope_root,
                    identity,
                    ..
                } => {
                    safe_file_identity::read_source_if_identity_matches(
                        path,
                        scope_root,
                        identity,
                        u64::MAX,
                    )?;
                    (path.clone(), identity.byte_count)
                }
            };
            let writable = !matches!(
                grant.kind,
                ResourceKindV1::ManagedRevision | ResourceKindV1::Executable
            ) && grant.allowed_verbs.iter().any(|verb| {
                matches!(
                    verb,
                    ResourceVerbV1::Create
                        | ResourceVerbV1::Replace
                        | ResourceVerbV1::Delete
                        | ResourceVerbV1::SetMetadata
                )
            });
            let (source_path, private_overlay) = if writable {
                let overlay = self
                    .process_root
                    .join(format!("world-overlay-{}", Uuid::new_v4()));
                fs::create_dir_all(&overlay)?;
                copy_private_tree(&source_path, &overlay, quota_bytes)?;
                (overlay.canonicalize()?, true)
            } else {
                (source_path, false)
            };
            let initial_bytes = if writable {
                scan_regular_tree(&source_path, quota_bytes)?
                    .values()
                    .map(|identity| identity.byte_count)
                    .sum()
            } else {
                0
            };
            mounts.push(ExecutionWorldMountV1 {
                handle_ref: grant.handle_ref.clone(),
                kind: grant.kind,
                source_path,
                mount_name: blake3::hash(grant.handle_ref.as_str().as_bytes()).to_hex()[..24]
                    .to_string(),
                writable,
                quota_bytes,
                allowed_verbs: grant.allowed_verbs.clone(),
                private_overlay,
                initial_bytes,
            });
        }
        self.world_leases.extend(requested);
        Ok(mounts)
    }

    /// Revalidates and records private overlay changes after a contained
    /// process exits. This produces resource state only; it cannot seal an
    /// OutputSlot or register logical lineage.
    pub(crate) fn release_execution_world_mounts(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        request_id: &EffectRequestIdV1,
        mounts: &[ExecutionWorldMountV1],
    ) -> AppResult<BTreeMap<String, (u64, String, u64)>> {
        let mut observations = BTreeMap::new();
        let mut first_error = None;
        for mount in mounts {
            let result = self.reconcile_execution_mount(objects, access, request_id, mount);
            match result {
                Ok(values) => observations.extend(values),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
            self.world_leases.remove(&mount.handle_ref);
            if mount.private_overlay {
                let _ = fs::remove_dir_all(&mount.source_path);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(observations)
    }

    fn reconcile_execution_mount(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        access: &ManagedResourceAccessV1,
        request_id: &EffectRequestIdV1,
        mount: &ExecutionWorldMountV1,
    ) -> AppResult<BTreeMap<String, (u64, String, u64)>> {
        let backing = self.backings.get_mut(&mount.handle_ref).ok_or_else(|| {
            AppError::InvalidInput("Execution world resource backing was revoked.".into())
        })?;
        validate_owner(backing.owner(), access)?;
        match backing {
            HostResourceBackingV1::ManagedRevision {
                acquisition,
                maximum_bytes,
                ..
            } => {
                let artifact = objects.resolve(acquisition, access.current.now)?;
                safe_file_identity::read_source_if_identity_matches(
                    &artifact.path,
                    &artifact.scope_root,
                    &artifact.identity,
                    *maximum_bytes,
                )?;
                Ok(BTreeMap::new())
            }
            HostResourceBackingV1::Executable {
                path,
                scope_root,
                identity,
                ..
            } => {
                safe_file_identity::read_source_if_identity_matches(
                    path,
                    scope_root,
                    identity,
                    u64::MAX,
                )?;
                Ok(BTreeMap::new())
            }
            HostResourceBackingV1::Workspace {
                acquisition,
                root,
                files,
                quota_bytes,
                ..
            } => {
                objects.resolve(acquisition, access.current.now)?;
                reconcile_private_overlay(
                    &mount.source_path,
                    root,
                    files,
                    *quota_bytes,
                    request_id,
                    &mount.allowed_verbs,
                )
            }
            HostResourceBackingV1::OutputSlot {
                root,
                files,
                quota_bytes,
                ..
            }
            | HostResourceBackingV1::Scratch {
                root,
                files,
                quota_bytes,
                ..
            } => reconcile_private_overlay(
                &mount.source_path,
                root,
                files,
                *quota_bytes,
                request_id,
                &mount.allowed_verbs,
            ),
        }
    }

    pub(crate) fn purge_bridge(&mut self, bridge_id: &str) -> usize {
        let handles = self
            .backings
            .iter()
            .filter(|(_, backing)| backing.owner().bridge_id == bridge_id)
            .map(|(handle, _)| handle.clone())
            .collect::<Vec<_>>();
        self.purge_handles(&handles);
        handles.len()
    }

    pub(crate) fn purge_run(&mut self, run_ref: &ManagedRunRefV1) -> usize {
        let handles = self
            .backings
            .iter()
            .filter(|(_, backing)| backing.owner().run_control_ref == *run_ref)
            .map(|(handle, _)| handle.clone())
            .collect::<Vec<_>>();
        self.purge_handles(&handles);
        handles.len()
    }

    pub(crate) fn purge_all(&mut self) -> usize {
        let handles = self.backings.keys().cloned().collect::<Vec<_>>();
        self.purge_handles(&handles);
        self.staged_payloads.clear();
        self.reads.clear();
        self.world_leases.clear();
        let _ = fs::remove_dir_all(&self.process_root);
        handles.len()
    }

    fn purge_handles(&mut self, handles: &[ResourceHandleRefV1]) {
        for handle in handles {
            if let Some(backing) = self.backings.remove(handle) {
                if let Some(root) = backing.private_root() {
                    let _ = fs::remove_dir_all(root);
                }
            }
        }
        self.staged_payloads
            .retain(|_, payload| !handles.contains(&payload.handle_ref));
        self.world_leases.retain(|handle| !handles.contains(handle));
    }

    fn allocate_root(&self, handle_ref: &ResourceHandleRefV1) -> AppResult<PathBuf> {
        fs::create_dir_all(&self.process_root)?;
        let root = self.process_root.join(
            blake3::hash(handle_ref.as_str().as_bytes())
                .to_hex()
                .to_string(),
        );
        fs::create_dir(&root).map_err(|_| {
            AppError::InvalidInput("Private managed resource root already exists.".into())
        })?;
        Ok(root.canonicalize()?)
    }

    fn insert_backing(
        &mut self,
        handle_ref: &ResourceHandleRefV1,
        backing: HostResourceBackingV1,
    ) -> AppResult<()> {
        if self.backings.contains_key(handle_ref) {
            if let Some(root) = backing.private_root() {
                let _ = fs::remove_dir_all(root);
            }
            return invalid("Managed resource handle is already physically bound.");
        }
        self.backings.insert(handle_ref.clone(), backing);
        Ok(())
    }

    fn apply_authorized(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        request: &EffectRequestV1,
        now: i64,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let EffectRequestKindV1::Resource(effect) = &request.effect else {
            return Ok(unavailable(
                "managed_resource_backend_has_no_process_or_network",
            ));
        };
        validate_managed_resource_selector(&effect.relative_selector)?;
        let backing = self.backings.get(&effect.handle_ref).ok_or_else(|| {
            AppError::InvalidInput("Managed resource backing is unavailable.".into())
        })?;
        if self.world_leases.contains(&effect.handle_ref) {
            return invalid("Managed resource is exclusively leased to an execution world.");
        }
        validate_request_owner(backing.owner(), request)?;
        match effect.verb {
            ResourceVerbV1::Inspect | ResourceVerbV1::Read => {
                self.apply_read(objects, request, now)
            }
            ResourceVerbV1::Create | ResourceVerbV1::Replace => {
                self.apply_write(objects, request, now)
            }
            ResourceVerbV1::Delete | ResourceVerbV1::SetMetadata => {
                Ok(unavailable("managed_resource_operation_unavailable"))
            }
        }
    }

    fn apply_read(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        request: &EffectRequestV1,
        now: i64,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let EffectRequestKindV1::Resource(effect) = &request.effect else {
            unreachable!();
        };
        let backing = self
            .backings
            .get_mut(&effect.handle_ref)
            .expect("validated backing");
        let (bytes, generation, digest) = match backing {
            HostResourceBackingV1::ManagedRevision {
                acquisition,
                maximum_bytes,
                ..
            } => {
                if effect.relative_selector != "." {
                    return invalid("ManagedRevisionHandle resolves only its exact revision root.");
                }
                let artifact = objects.resolve(acquisition, now)?;
                let bytes = safe_file_identity::read_source_if_identity_matches(
                    &artifact.path,
                    &artifact.scope_root,
                    &artifact.identity,
                    *maximum_bytes,
                )?;
                let digest = artifact.identity.digest;
                (bytes, 1, digest)
            }
            HostResourceBackingV1::Workspace {
                acquisition,
                root,
                files,
                ..
            } => {
                objects.resolve(acquisition, now)?;
                read_private_file(root, files, &effect.relative_selector, request)?
            }
            HostResourceBackingV1::OutputSlot { root, files, .. }
            | HostResourceBackingV1::Scratch { root, files, .. } => {
                read_private_file(root, files, &effect.relative_selector, request)?
            }
            HostResourceBackingV1::Executable { .. } => {
                return invalid("Executable handles are usable only by a verified execution world.")
            }
        };
        if bytes.len() as u64 > request.requested_budget_slice.read_bytes {
            return invalid("Managed resource read exceeded its reserved budget.");
        }
        self.reads.insert(
            request.request_id.clone(),
            HostResourceReadV1 {
                request_id: request.request_id.clone(),
                bytes: bytes.clone(),
                generation,
                content_digest: digest.clone(),
            },
        );
        Ok(allowed_resource(
            &effect.handle_ref,
            generation,
            digest,
            bytes.len() as u64,
            "managed_resource_read",
        ))
    }

    fn apply_write(
        &mut self,
        objects: &mut ManagedObjectBindingService,
        request: &EffectRequestV1,
        now: i64,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let EffectRequestKindV1::Resource(effect) = &request.effect else {
            unreachable!();
        };
        let Some(content_digest) = effect.value_digest.as_deref() else {
            return invalid("Managed resource write requires exact staged content identity.");
        };
        let key = staged_key(&request.run_control_ref, &effect.handle_ref, content_digest);
        let payload = self.staged_payloads.remove(&key).ok_or_else(|| {
            AppError::InvalidInput("Exact staged resource payload is unavailable.".into())
        })?;
        validate_request_owner(&payload.owner, request)?;
        if payload.handle_ref != effect.handle_ref
            || payload.content_digest != content_digest
            || payload.bytes.len() as u64 > request.requested_budget_slice.write_bytes
        {
            return invalid("Staged resource payload context or budget is mismatched.");
        }
        let backing = self
            .backings
            .get_mut(&effect.handle_ref)
            .expect("validated backing");
        if let HostResourceBackingV1::Workspace { acquisition, .. } = backing {
            objects.resolve(acquisition, now)?;
        }
        let (root, files, quota_bytes) = match backing {
            HostResourceBackingV1::ManagedRevision { .. } => {
                return invalid("Authoritative managed revisions cannot be mutated in place.")
            }
            HostResourceBackingV1::Executable { .. } => {
                return invalid("Executable resources cannot be mutated by Worker authority.")
            }
            HostResourceBackingV1::Workspace {
                root,
                files,
                quota_bytes,
                ..
            }
            | HostResourceBackingV1::OutputSlot {
                root,
                files,
                quota_bytes,
                ..
            }
            | HostResourceBackingV1::Scratch {
                root,
                files,
                quota_bytes,
                ..
            } => (root, files, *quota_bytes),
        };
        let existing = files.get(&effect.relative_selector);
        let (expected_generation, expected_digest) =
            expected_generation(request, &effect.handle_ref)?;
        match (effect.verb, existing) {
            (ResourceVerbV1::Create, None)
                if expected_generation == 0 && expected_digest.is_none() => {}
            (ResourceVerbV1::Replace, Some(file))
                if expected_generation == file.generation
                    && expected_digest.as_deref() == Some(file.identity.digest.as_str())
                    && !file.sealed => {}
            _ => return invalid("Managed resource generation or write verb is stale."),
        }
        let current_bytes = files
            .iter()
            .filter(|(selector, _)| selector.as_str() != effect.relative_selector)
            .map(|(_, file)| file.identity.byte_count)
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| AppError::InvalidInput("Managed resource quota overflowed.".into()))?;
        if current_bytes
            .checked_add(payload.bytes.len() as u64)
            .is_none_or(|total| total > quota_bytes)
        {
            return invalid("Managed resource private quota is exhausted.");
        }
        let generation = expected_generation
            .checked_add(1)
            .ok_or_else(|| AppError::InvalidInput("Resource generation overflowed.".into()))?;
        let expected_identity = existing.map(|file| &file.identity);
        let mut file = write_private_file(
            root,
            &effect.relative_selector,
            expected_identity,
            &payload.bytes,
            generation,
        )?;
        file.last_request_id = Some(request.request_id.clone());
        let digest = file.identity.digest.clone();
        let bytes = file.identity.byte_count;
        files.insert(effect.relative_selector.clone(), file);
        Ok(allowed_resource(
            &effect.handle_ref,
            generation,
            digest,
            bytes,
            "managed_resource_write",
        ))
    }
}

impl Drop for ManagedResourceResolverV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.process_root);
    }
}

/// Resource-only Host backend. The existing enforcer calls this only after
/// claiming ordered intent, validating exact authority, and reserving budget.
/// Process and Network requests remain explicitly unavailable.
pub(crate) struct HostManagedResourceBackendV1<'a> {
    resolver: &'a mut ManagedResourceResolverV1,
    objects: &'a mut ManagedObjectBindingService,
    now: i64,
}

impl<'a> HostManagedResourceBackendV1<'a> {
    pub(crate) fn new(
        resolver: &'a mut ManagedResourceResolverV1,
        objects: &'a mut ManagedObjectBindingService,
        now: i64,
    ) -> Self {
        Self {
            resolver,
            objects,
            now,
        }
    }
}

impl HostEffectBackendV1 for HostManagedResourceBackendV1<'_> {
    fn apply(&mut self, request: &EffectRequestV1) -> BackendApplyV1 {
        let outcome = self
            .resolver
            .apply_authorized(self.objects, request, self.now)
            .unwrap_or_else(|_| BackendEffectOutcomeV1 {
                decision: EffectDecisionV1::Denied,
                actual_effect_summary: "managed_resource_resolution_denied".into(),
                facts: EffectFactsV1::None,
            });
        BackendApplyV1::Completed(outcome)
    }
}

fn validate_attachment(
    authority: &EffectAuthorityStateV1,
    access: &ManagedResourceAccessV1,
    handle_ref: &ResourceHandleRefV1,
    kind: ResourceKindV1,
) -> AppResult<ResourceGrantV1> {
    authority.validate_resource_attachment(
        handle_ref,
        kind,
        &access.envelope_ref,
        &access.run_control_ref,
        &access.context,
        &access.current,
    )
}

fn owner(access: &ManagedResourceAccessV1) -> AppResult<ExactOwnerV1> {
    Ok(ExactOwnerV1 {
        envelope_ref: access.envelope_ref.clone(),
        run_control_ref: access.run_control_ref.clone(),
        context_ref: access.context.context_ref()?,
        bridge_id: access.context.bridge_id.clone(),
    })
}

fn validate_owner(owner: &ExactOwnerV1, access: &ManagedResourceAccessV1) -> AppResult<()> {
    if owner.envelope_ref != access.envelope_ref
        || owner.run_control_ref != access.run_control_ref
        || owner.context_ref != access.context.context_ref()?
    {
        return invalid("Managed resource owner context was substituted.");
    }
    Ok(())
}

fn validate_request_owner(owner: &ExactOwnerV1, request: &EffectRequestV1) -> AppResult<()> {
    if owner.envelope_ref != request.envelope_ref
        || owner.run_control_ref != request.run_control_ref
        || owner.context_ref != request.context.context_ref()?
    {
        return invalid("Managed resource request owner context was substituted.");
    }
    Ok(())
}

fn validate_exact_input(
    context: &AuthorityContextV1,
    acquisition: &ManagedObjectAcquisition,
) -> AppResult<()> {
    if acquisition.object.host_ref != context.host_ref
        || acquisition.binding.host_ref != context.host_ref
        || acquisition.binding.logical_object_id != acquisition.object.logical_object_id
        || acquisition.binding.revision != acquisition.object.revision
        || !context.input_revisions.iter().any(|input| {
            input.logical_object_id == acquisition.object.logical_object_id
                && input.revision == acquisition.object.revision
                && input.host_ref == acquisition.object.host_ref
        })
    {
        return invalid("Managed resource does not match an exact Host-bound input revision.");
    }
    Ok(())
}

fn validate_quota(quota: u64, ceiling: u64) -> AppResult<()> {
    if quota == 0 || quota > ceiling {
        return invalid("Managed resource quota exceeds its resource grant.");
    }
    Ok(())
}

pub(crate) fn validate_managed_resource_selector(selector: &str) -> AppResult<()> {
    if selector.is_empty()
        || selector.len() > MAX_SELECTOR_BYTES
        || selector.contains('\0')
        || selector.contains('\\')
        || selector.to_ascii_lowercase().starts_with("file:")
    {
        return invalid("Managed resource selector is invalid.");
    }
    if selector == "." {
        return Ok(());
    }
    let path = Path::new(selector);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid("Managed resource selector must be normalized and relative.");
    }
    Ok(())
}

fn identity_ref(
    domain: &str,
    acquisition: &ManagedObjectAcquisition,
    artifact: &ResolvedManagedArtifact,
) -> AppResult<String> {
    domain_hash(
        domain,
        &(
            &acquisition.object.logical_object_id,
            acquisition.object.revision,
            &acquisition.object.host_ref,
            &acquisition.binding.binding_ref,
            &artifact.identity.digest,
            artifact.identity.byte_count,
        ),
    )
}

fn staged_key(run_ref: &ManagedRunRefV1, handle_ref: &ResourceHandleRefV1, digest: &str) -> String {
    format!("{}\0{}\0{}", run_ref.as_str(), handle_ref.as_str(), digest)
}

fn expected_generation(
    request: &EffectRequestV1,
    handle_ref: &ResourceHandleRefV1,
) -> AppResult<(u64, Option<String>)> {
    let matches = request
        .preconditions
        .iter()
        .filter_map(|condition| match condition {
            EffectPreconditionV1::ResourceGeneration {
                handle_ref: observed,
                generation,
                digest,
            } if observed == handle_ref => Some((*generation, digest.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok((0, None)),
        [value] => Ok((value.0, Some(value.1.clone()))),
        _ => invalid("Managed resource generation precondition is ambiguous."),
    }
}

fn read_private_file(
    root: &Path,
    files: &HashMap<String, PrivateFileV1>,
    selector: &str,
    request: &EffectRequestV1,
) -> AppResult<(Vec<u8>, u64, String)> {
    let file = files.get(selector).ok_or_else(|| {
        AppError::InvalidInput("Managed resource selector is unavailable.".into())
    })?;
    let bytes = safe_file_identity::read_source_if_identity_matches(
        &file.path,
        root,
        &file.identity,
        request.requested_budget_slice.read_bytes,
    )?;
    Ok((bytes, file.generation, file.identity.digest.clone()))
}

fn unsafe_directory_metadata(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0;
    }
    #[cfg(not(windows))]
    false
}

fn scan_regular_tree(root: &Path, quota_bytes: u64) -> AppResult<BTreeMap<String, SourceIdentity>> {
    let canonical_root = root.canonicalize().map_err(|_| {
        AppError::InvalidInput("Private managed resource root is unavailable.".into())
    })?;
    let mut pending = vec![canonical_root.clone()];
    let mut observed = BTreeMap::new();
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let metadata = fs::symlink_metadata(&directory)?;
        if unsafe_directory_metadata(&metadata) || !directory.starts_with(&canonical_root) {
            return invalid("Execution world resource tree contains an unsafe directory.");
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return invalid(
                    "Execution world resource tree contains a symlink or reparse point.",
                );
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return invalid("Execution world resource tree contains a special file.");
            }
            let relative = path.strip_prefix(&canonical_root).map_err(|_| {
                AppError::InvalidInput("Execution world resource escaped its private root.".into())
            })?;
            let selector = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            validate_managed_resource_selector(&selector)?;
            let remaining = quota_bytes.saturating_sub(total);
            let identity =
                safe_file_identity::capture_source_identity(&path, &canonical_root, remaining)?;
            total = total.checked_add(identity.byte_count).ok_or_else(|| {
                AppError::InvalidInput("Execution world resource quota overflowed.".into())
            })?;
            if total > quota_bytes || observed.insert(selector, identity).is_some() {
                return invalid(
                    "Execution world resource tree exceeds quota or aliases a selector.",
                );
            }
        }
    }
    Ok(observed)
}

fn validate_private_tree(
    root: &Path,
    files: &HashMap<String, PrivateFileV1>,
    quota_bytes: u64,
) -> AppResult<()> {
    let observed = scan_regular_tree(root, quota_bytes)?;
    if observed.len() != files.len()
        || files.iter().any(|(selector, file)| {
            observed.get(selector) != Some(&file.identity) || file.path != root.join(selector)
        })
    {
        return invalid("Private managed resource identity is stale, changed, or aliased.");
    }
    Ok(())
}

fn copy_private_tree(source: &Path, destination: &Path, quota_bytes: u64) -> AppResult<()> {
    let observed = scan_regular_tree(source, quota_bytes)?;
    for (selector, identity) in observed {
        let bytes = safe_file_identity::read_source_if_identity_matches(
            &source.join(&selector),
            source,
            &identity,
            quota_bytes,
        )?;
        write_private_file(destination, &selector, None, &bytes, 1)?;
    }
    Ok(())
}

fn reconcile_private_overlay(
    overlay: &Path,
    destination_root: &Path,
    files: &mut HashMap<String, PrivateFileV1>,
    quota_bytes: u64,
    request_id: &EffectRequestIdV1,
    allowed_verbs: &BTreeSet<ResourceVerbV1>,
) -> AppResult<BTreeMap<String, (u64, String, u64)>> {
    let observed = scan_regular_tree(overlay, quota_bytes)?;
    let deleted = files
        .keys()
        .filter(|selector| !observed.contains_key(*selector))
        .cloned()
        .collect::<Vec<_>>();
    if !deleted.is_empty() && !allowed_verbs.contains(&ResourceVerbV1::Delete) {
        return invalid("Execution world attempted an unauthorized resource deletion.");
    }
    for (selector, identity) in &observed {
        match files.get(selector) {
            None if !allowed_verbs.contains(&ResourceVerbV1::Create) => {
                return invalid("Execution world attempted an unauthorized resource creation.")
            }
            Some(previous)
                if (previous.identity.digest != identity.digest
                    || previous.identity.byte_count != identity.byte_count)
                    && !allowed_verbs.contains(&ResourceVerbV1::Replace) =>
            {
                return invalid("Execution world attempted an unauthorized resource replacement.")
            }
            _ => {}
        }
    }

    for selector in deleted {
        let previous = files.remove(&selector).expect("validated selector");
        let current = safe_file_identity::capture_source_identity(
            &previous.path,
            destination_root,
            previous.identity.byte_count,
        )?;
        if current != previous.identity {
            return invalid("Managed resource changed before overlay deletion could commit.");
        }
        fs::remove_file(previous.path)?;
    }

    let mut facts = BTreeMap::new();
    for (selector, identity) in observed {
        let changed = files.get(&selector).is_none_or(|previous| {
            previous.identity.digest != identity.digest
                || previous.identity.byte_count != identity.byte_count
        });
        if changed {
            let bytes = safe_file_identity::read_source_if_identity_matches(
                &overlay.join(&selector),
                overlay,
                &identity,
                quota_bytes,
            )?;
            let previous = files.get(&selector);
            let generation = previous.map_or(1, |file| file.generation.saturating_add(1));
            let mut committed = write_private_file(
                destination_root,
                &selector,
                previous.map(|file| &file.identity),
                &bytes,
                generation,
            )?;
            committed.last_request_id = Some(request_id.clone());
            files.insert(selector.clone(), committed);
        }
        let file = files.get(&selector).expect("observed file committed");
        facts.insert(
            selector,
            (
                file.generation,
                file.identity.digest.clone(),
                file.identity.byte_count,
            ),
        );
    }
    Ok(facts)
}

fn write_private_file(
    root: &Path,
    selector: &str,
    expected_identity: Option<&SourceIdentity>,
    bytes: &[u8],
    generation: u64,
) -> AppResult<PrivateFileV1> {
    validate_managed_resource_selector(selector)?;
    if selector == "." {
        return invalid("A mutable private file requires a relative selector.");
    }
    let canonical_root = root.canonicalize().map_err(|_| {
        AppError::InvalidInput("Private managed resource root is unavailable.".into())
    })?;
    let relative = Path::new(selector);
    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut parent = canonical_root.clone();
    for component in parent_relative.components() {
        let Component::Normal(name) = component else {
            return invalid("Managed resource selector escaped its private root.");
        };
        parent.push(name);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) => {
                if unsafe_directory_metadata(&metadata) {
                    return invalid("Managed resource selector crosses an unsafe directory.");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&parent)?,
            Err(error) => return Err(error.into()),
        }
        if !parent.canonicalize()?.starts_with(&canonical_root) {
            return invalid("Managed resource selector escaped its private root.");
        }
    }
    let destination = canonical_root.join(relative);
    match (expected_identity, fs::symlink_metadata(&destination)) {
        (None, Ok(_)) => return invalid("Managed resource create target already exists."),
        (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        (None, Err(error)) => return Err(error.into()),
        (Some(expected), Ok(metadata)) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return invalid("Managed resource replace target is unsafe.");
            }
            let observed = safe_file_identity::capture_source_identity(
                &destination,
                &canonical_root,
                expected.byte_count,
            )?;
            if &observed != expected {
                return invalid("Managed resource replace identity is stale or aliased.");
            }
        }
        (Some(_), Err(_)) => return invalid("Managed resource replace target is unavailable."),
    }
    let temporary = parent.join(format!(".pastey-write-{}", Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    drop(output);
    if expected_identity.is_some() {
        // The existing target was identity-checked above. Replacing the
        // directory entry does not follow its target and breaks old aliases.
        #[cfg(windows)]
        fs::remove_file(&destination)?;
    }
    fs::rename(&temporary, &destination)?;
    let identity = safe_file_identity::capture_source_identity(
        &destination,
        &canonical_root,
        bytes.len() as u64,
    )?;
    if identity.digest != digest_bytes(bytes) || identity.byte_count != bytes.len() as u64 {
        return invalid("Managed resource write identity is indeterminate.");
    }
    Ok(PrivateFileV1 {
        path: destination,
        identity,
        generation,
        sealed: false,
        lineage_registered: false,
        last_request_id: None,
    })
}

fn digest_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn domain_hash<T: Serialize>(domain: &str, value: &T) -> AppResult<String> {
    let canonical = serde_json::to_vec(value)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(&(canonical.len() as u64).to_be_bytes());
    hasher.update(&canonical);
    Ok(format!("{domain}:{}", hasher.finalize().to_hex()))
}

fn allowed_resource(
    handle_ref: &ResourceHandleRefV1,
    generation: u64,
    content_digest: String,
    bytes: u64,
    summary: &str,
) -> BackendEffectOutcomeV1 {
    BackendEffectOutcomeV1 {
        decision: EffectDecisionV1::Allowed,
        actual_effect_summary: summary.into(),
        facts: EffectFactsV1::Resource {
            handle_ref: handle_ref.clone(),
            generation,
            content_digest,
            bytes,
        },
    }
}

fn unavailable(summary: &str) -> BackendEffectOutcomeV1 {
    BackendEffectOutcomeV1 {
        decision: EffectDecisionV1::Unavailable,
        actual_effect_summary: summary.into(),
        facts: EffectFactsV1::None,
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    #[cfg(target_os = "macos")]
    use std::time::Duration;

    use super::*;
    #[cfg(target_os = "macos")]
    use crate::execution_world::HostManagedProcessBackendV1;
    use crate::{
        effect_authority::{
            compile_effect_envelope, execution_world_ref_for, lower_tool_request,
            AuthorityCeilingV1, ConfinementPropertyV1, EffectBoundV1, EffectBudgetsV1,
            EffectCapabilityV1, EffectEnvelopeCompileRequestV1, ExecutionWorldGrantV1,
            ExecutionWorldRefV1, ManagedInputRevisionV1, ManagedSemanticOperationV1,
            NetworkAuthorityV1, ProcessEffectV1, ProcessVerbV1, ResourceEffectV1,
            ResourceGrantSpecV1, ResultContractV1, StepWorkDescriptorV1, ToolEffectIntentV1,
            ToolRequestV1, EFFECT_AUTHORITY_VERSION,
        },
        execution_world::{ExecutionWorldServiceV1, ManagedProcessInvocationV1},
        host_identity::{HostRef, HostSessionBinding, PlanParticipantRef},
        managed_objects::{HostArtifactAcquisition, ManagedObjectAcquisitionKind},
    };

    struct Fixture {
        root: PathBuf,
        source_path: PathBuf,
        objects: ManagedObjectBindingService,
        acquisition: ManagedObjectAcquisition,
        authority: EffectAuthorityStateV1,
        resolver: ManagedResourceResolverV1,
        access: ManagedResourceAccessV1,
        managed: ResourceHandleRefV1,
        workspace: ResourceHandleRefV1,
        output: ResourceHandleRefV1,
        scratch: ResourceHandleRefV1,
        executable: ResourceHandleRefV1,
        world: ExecutionWorldRefV1,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn total_budget() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 32,
            read_bytes: 32 * 1024,
            write_bytes: 32 * 1024,
            process_spawns: 8,
            process_signals: 8,
            cpu_millis: 8_000,
            memory_byte_millis: 512 * 1024 * 1024 * 8_000,
            wall_millis: 8_000,
            ..EffectBudgetsV1::default()
        }
    }

    fn request_budget(read: u64, write: u64) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            read_bytes: read,
            write_bytes: write,
            ..EffectBudgetsV1::default()
        }
    }

    fn fixture() -> Fixture {
        let now = crate::storage::now_ts();
        let root = std::env::temp_dir().join(format!("pastey-step5-{}", Uuid::new_v4()));
        let source_root = root.join("source");
        fs::create_dir_all(&source_root).unwrap();
        let source_path = source_root.join("input.txt");
        fs::write(&source_path, b"authoritative revision N").unwrap();

        let local = HostRef::from_device_id("phase5-step5-local").unwrap();
        let peer = HostRef::from_device_id("phase5-step5-peer").unwrap();
        let binding = HostSessionBinding::new(
            "bridge-step5",
            local.clone(),
            peer,
            "local-session-step5",
            "peer-session-step5",
            "peer-route-step5",
            now + 900,
        )
        .unwrap();
        let mut objects = ManagedObjectBindingService::new(local.clone());
        let acquisition = objects
            .acquire_new(
                HostArtifactAcquisition {
                    kind: ManagedObjectAcquisitionKind::LocalSelection,
                    source_ref: "source-step5".into(),
                    bridge_id: Some("bridge-step5".into()),
                    path: source_path.clone(),
                    scope_root: source_root,
                    display_name: "input.txt".into(),
                    media_type: "text/plain".into(),
                    expires_at: now + 800,
                    app_owned_temporary: false,
                },
                now,
            )
            .unwrap();
        let artifact = objects.resolve(&acquisition, now).unwrap();
        let mut resolver = ManagedResourceResolverV1::new(root.join("private"));
        let managed_identity =
            ManagedResourceResolverV1::managed_revision_identity_ref(&acquisition, &artifact)
                .unwrap();
        let workspace_identity =
            ManagedResourceResolverV1::workspace_identity_ref(&acquisition, &artifact).unwrap();
        #[cfg(not(windows))]
        let executable_spec = ExecutableBindingSpecV1 {
            executable_path: PathBuf::from("/bin/sh"),
            scope_root: PathBuf::from("/bin"),
        };
        #[cfg(windows)]
        let executable_spec = {
            let executable_path = std::env::current_exe().unwrap();
            let scope_root = executable_path.parent().unwrap().to_owned();
            ExecutableBindingSpecV1 {
                executable_path,
                scope_root,
            }
        };
        let executable_identity =
            ManagedResourceResolverV1::executable_identity_ref(&executable_spec).unwrap();
        let context = AuthorityContextV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            bridge_id: "bridge-step5".into(),
            plan_id: "plan-step5".into(),
            revision_id: "plan-revision-step5".into(),
            revision_hash: "plan-revision-hash-step5".into(),
            approval_id: "approval-step5".into(),
            attempt_id: "attempt-step5".into(),
            step_id: "transform-step5".into(),
            semantic_operation: ManagedSemanticOperationV1::Transform,
            participant_ref: PlanParticipantRef::for_host("plan-step5", &local).unwrap(),
            host_ref: local.clone(),
            admission_ref: "admission-step5".into(),
            session_binding_ref: binding.binding_ref.clone(),
            input_revisions: vec![ManagedInputRevisionV1 {
                logical_object_id: acquisition.object.logical_object_id.clone(),
                revision: acquisition.object.revision,
                host_ref: local.clone(),
            }],
            issued_at: now - 1,
            expires_at: now + 700,
        };
        let mut authority = EffectAuthorityStateV1::default();
        let draft = authority.begin_run(context.clone()).unwrap();
        let mut mint = |kind, identity: &str, verbs: &[ResourceVerbV1]| {
            authority
                .mint_resource_grant(
                    &draft,
                    ResourceGrantSpecV1 {
                        host_ref: local.clone(),
                        kind,
                        safe_identity_ref: identity.into(),
                        selector_prefix: ".".into(),
                        allowed_verbs: verbs.iter().copied().collect(),
                        budgets: total_budget(),
                        expires_at: now + 700,
                    },
                )
                .unwrap()
        };
        let managed = mint(
            ResourceKindV1::ManagedRevision,
            &managed_identity,
            &[ResourceVerbV1::Inspect, ResourceVerbV1::Read],
        );
        let workspace = mint(
            ResourceKindV1::Workspace,
            &workspace_identity,
            &[
                ResourceVerbV1::Inspect,
                ResourceVerbV1::Read,
                ResourceVerbV1::Create,
                ResourceVerbV1::Replace,
            ],
        );
        let output = mint(
            ResourceKindV1::OutputSlot,
            "output-slot-safe-identity-step5",
            &[
                ResourceVerbV1::Inspect,
                ResourceVerbV1::Read,
                ResourceVerbV1::Create,
                ResourceVerbV1::Replace,
            ],
        );
        let scratch = mint(
            ResourceKindV1::Scratch,
            "scratch-safe-identity-step5",
            &[
                ResourceVerbV1::Inspect,
                ResourceVerbV1::Read,
                ResourceVerbV1::Create,
                ResourceVerbV1::Replace,
            ],
        );
        let executable = mint(
            ResourceKindV1::Executable,
            &executable_identity,
            &[ResourceVerbV1::Inspect, ResourceVerbV1::Read],
        );
        let resources = vec![
            managed.clone(),
            workspace.clone(),
            output.clone(),
            scratch.clone(),
            executable.clone(),
        ];
        let availability = ExecutionWorldServiceV1::platform_availability();
        let world_identity = availability.identity_digest.as_str();
        let world = ExecutionWorldGrantV1 {
            world_ref: execution_world_ref_for(&draft, world_identity).unwrap(),
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            world_identity_digest: world_identity.into(),
            mounted_resources: resources
                .iter()
                .filter(|grant| grant.kind != ResourceKindV1::Executable)
                .map(|grant| grant.handle_ref.clone())
                .collect(),
            executable_resources: [executable.handle_ref.clone()].into_iter().collect(),
            required_properties: [
                ConfinementPropertyV1::NoAmbientFilesystem,
                ConfinementPropertyV1::EmptyEnvironment,
                ConfinementPropertyV1::NoInheritedDescriptors,
                ConfinementPropertyV1::ContainedProcessTree,
                ConfinementPropertyV1::NoDaemonSurvival,
                ConfinementPropertyV1::NoRawNetwork,
            ]
            .into_iter()
            .collect(),
            budgets: total_budget(),
            expires_at: now + 700,
        };
        let effect_bounds = [
            ResourceVerbV1::Inspect,
            ResourceVerbV1::Read,
            ResourceVerbV1::Create,
            ResourceVerbV1::Replace,
        ]
        .into_iter()
        .map(|verb| EffectBoundV1 {
            capability: EffectCapabilityV1::Resource(verb),
            max_per_request: request_budget(4096, 4096),
        })
        .collect::<Vec<_>>();
        let mut effect_bounds = effect_bounds;
        effect_bounds.extend([ProcessVerbV1::Spawn, ProcessVerbV1::Signal].map(|verb| {
            EffectBoundV1 {
                capability: EffectCapabilityV1::Process(verb),
                max_per_request: total_budget(),
            }
        }));
        let ceiling = AuthorityCeilingV1 {
            context_ref: draft.context_ref.clone(),
            source_snapshot_ref: "step5-authority-ceiling".into(),
            resources,
            world,
            effect_bounds,
            budgets: total_budget(),
            network: NetworkAuthorityV1::Denied,
            expires_at: now + 700,
        };
        let envelope = compile_effect_envelope(EffectEnvelopeCompileRequestV1 {
            context: context.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            semantic_ceiling: ceiling.clone(),
            admission_ceiling: ceiling.clone(),
            host_policy_ceiling: ceiling.clone(),
            confinement_ceiling: ceiling,
            host_policy_snapshot_ref: "step5-host-policy".into(),
            result_contract: ResultContractV1::Transform {
                input: context.input_revisions[0].clone(),
                output_revision: acquisition.object.revision + 1,
                output_slot: output.handle_ref.clone(),
            },
        })
        .unwrap();
        authority.install_envelope(draft, envelope.clone()).unwrap();
        authority
            .activate_run(&envelope.run_control_ref, now)
            .unwrap();
        let world_ref = envelope.world.world_ref.clone();
        let access = ManagedResourceAccessV1 {
            envelope_ref: envelope.envelope_ref,
            run_control_ref: envelope.run_control_ref,
            context,
            current: CurrentHostAuthorityV1 {
                session_binding: binding,
                bridge_active: true,
                burned: false,
                disconnected: false,
                restarted: false,
                now,
            },
        };
        resolver
            .bind_executable(&authority, &access, &executable.handle_ref, executable_spec)
            .unwrap();
        Fixture {
            root,
            source_path,
            objects,
            acquisition,
            authority,
            resolver,
            access,
            managed: managed.handle_ref,
            workspace: workspace.handle_ref,
            output: output.handle_ref,
            scratch: scratch.handle_ref,
            executable: executable.handle_ref,
            world: world_ref,
        }
    }

    #[allow(clippy::too_many_arguments)] // Compact test-only request matrix helper.
    fn request(
        fixture: &Fixture,
        sequence: u64,
        verb: ResourceVerbV1,
        handle_ref: ResourceHandleRefV1,
        selector: &str,
        value_digest: Option<String>,
        preconditions: Vec<EffectPreconditionV1>,
        budget: EffectBudgetsV1,
    ) -> EffectRequestV1 {
        lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: fixture.access.context.clone(),
                envelope_ref: fixture.access.envelope_ref.clone(),
                run_control_ref: fixture.access.run_control_ref.clone(),
                first_sequence: sequence,
            },
            &ToolRequestV1 {
                tool_name: "synthetic-resource-tool".into(),
                adapter_version_ref: "synthetic-resource-adapter-v1".into(),
                intents: vec![ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                        verb,
                        handle_ref,
                        relative_selector: selector.into(),
                        value_digest,
                    }),
                    requested_budget_slice: budget,
                    preconditions,
                }],
            },
        )
        .unwrap()
        .remove(0)
    }

    fn enforce(fixture: &mut Fixture, request: &EffectRequestV1) -> EffectEvidenceV1 {
        let mut backend = HostManagedResourceBackendV1::new(
            &mut fixture.resolver,
            &mut fixture.objects,
            fixture.access.current.now,
        );
        fixture
            .authority
            .enforce(request, &fixture.access.current, &mut backend)
            .unwrap()
    }

    #[test]
    fn selectors_reject_absolute_traversal_and_symlink_escape() {
        for selector in ["../escape", "/absolute", "a/../../escape", "file:secret"] {
            let mut fixture = fixture();
            fixture
                .resolver
                .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
                .unwrap();
            let req = request(
                &fixture,
                0,
                ResourceVerbV1::Read,
                fixture.output.clone(),
                selector,
                None,
                vec![],
                request_budget(1, 0),
            );
            assert_eq!(
                enforce(&mut fixture, &req).decision,
                EffectDecisionV1::Denied
            );
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let mut fixture = fixture();
            fixture
                .resolver
                .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
                .unwrap();
            let root = match fixture.resolver.backings.get(&fixture.output).unwrap() {
                HostResourceBackingV1::OutputSlot { root, .. } => root.clone(),
                _ => unreachable!(),
            };
            symlink(&fixture.root, root.join("escape")).unwrap();
            let bytes = b"no escape".to_vec();
            let digest = digest_bytes(&bytes);
            fixture
                .resolver
                .stage_write_payload(
                    &fixture.authority,
                    &fixture.access,
                    &fixture.output,
                    &digest,
                    bytes,
                )
                .unwrap();
            let req = request(
                &fixture,
                0,
                ResourceVerbV1::Create,
                fixture.output.clone(),
                "escape/file.txt",
                Some(digest),
                vec![],
                request_budget(0, 64),
            );
            assert_eq!(
                enforce(&mut fixture, &req).decision,
                EffectDecisionV1::Denied
            );
        }
    }

    #[test]
    fn managed_revision_revalidates_identity_and_is_never_mutable() {
        let mut fixture = fixture();
        fixture
            .resolver
            .bind_managed_revision(
                &fixture.authority,
                &mut fixture.objects,
                &fixture.access,
                &fixture.managed,
                fixture.acquisition.clone(),
            )
            .unwrap();
        assert!(fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.managed,
                &digest_bytes(b"mutation"),
                b"mutation".to_vec(),
            )
            .is_err());
        fs::write(&fixture.source_path, b"changed after safe binding").unwrap();
        let req = request(
            &fixture,
            0,
            ResourceVerbV1::Read,
            fixture.managed.clone(),
            ".",
            None,
            vec![],
            request_budget(128, 0),
        );
        assert_eq!(
            enforce(&mut fixture, &req).decision,
            EffectDecisionV1::Denied
        );
    }

    #[test]
    fn workspace_is_a_private_overlay_and_stale_generation_is_rejected() {
        let mut fixture = fixture();
        fixture
            .resolver
            .bind_workspace(
                &fixture.authority,
                &mut fixture.objects,
                &fixture.access,
                &fixture.workspace,
                WorkspaceBindingSpecV1 {
                    acquisition: fixture.acquisition.clone(),
                    initial_selector: "project/input.txt".into(),
                    quota_bytes: 4096,
                },
            )
            .unwrap();
        let original = fs::read(&fixture.source_path).unwrap();
        let replacement = b"private overlay change".to_vec();
        let replacement_digest = digest_bytes(&replacement);
        fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.workspace,
                &replacement_digest,
                replacement,
            )
            .unwrap();
        let req = request(
            &fixture,
            0,
            ResourceVerbV1::Replace,
            fixture.workspace.clone(),
            "project/input.txt",
            Some(replacement_digest),
            vec![EffectPreconditionV1::ResourceGeneration {
                handle_ref: fixture.workspace.clone(),
                generation: 1,
                digest: digest_bytes(&original),
            }],
            request_budget(0, 128),
        );
        assert_eq!(
            enforce(&mut fixture, &req).decision,
            EffectDecisionV1::Allowed
        );
        assert_eq!(fs::read(&fixture.source_path).unwrap(), original);
        fs::write(&fixture.source_path, b"stale authoritative identity").unwrap();
        let stale_read = request(
            &fixture,
            1,
            ResourceVerbV1::Read,
            fixture.workspace.clone(),
            "project/input.txt",
            None,
            vec![],
            request_budget(128, 0),
        );
        assert_eq!(
            enforce(&mut fixture, &stale_read).decision,
            EffectDecisionV1::Denied
        );
    }

    #[test]
    fn cross_run_envelope_and_session_substitution_fail_closed() {
        let mut primary = fixture();
        let mut substituted = primary.access.clone();
        substituted.current.session_binding.binding_ref = "wrong-binding".into();
        assert!(primary
            .resolver
            .provision_scratch(&primary.authority, &substituted, &primary.scratch, 4096,)
            .is_err());

        let other = fixture();
        assert!(primary
            .resolver
            .provision_output_slot(&primary.authority, &other.access, &primary.output, 4096,)
            .is_err());
    }

    #[test]
    fn output_slot_generation_seals_to_bounded_evidence_only() {
        let mut fixture = fixture();
        fixture
            .resolver
            .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
            .unwrap();
        let bytes = b"candidate N plus one".to_vec();
        let digest = digest_bytes(&bytes);
        fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.output,
                &digest,
                bytes,
            )
            .unwrap();
        let req = request(
            &fixture,
            0,
            ResourceVerbV1::Create,
            fixture.output.clone(),
            "result.bin",
            Some(digest.clone()),
            vec![],
            request_budget(0, 128),
        );
        let evidence = enforce(&mut fixture, &req);
        assert_eq!(evidence.decision, EffectDecisionV1::Allowed);
        let sealed = fixture
            .resolver
            .seal_output_slot(
                &fixture.authority,
                &fixture.access,
                &fixture.output,
                "result.bin",
                &evidence,
            )
            .unwrap();
        assert_eq!(sealed.generation, 1);
        assert_eq!(sealed.content_digest, digest);
        let encoded = serde_json::to_string(&sealed).unwrap();
        assert!(!encoded.contains("logicalObjectId"));
        assert!(!encoded.contains("outputRevision"));
        assert!(!encoded.contains(fixture.root.to_string_lossy().as_ref()));
    }

    #[test]
    fn scratch_is_quota_bounded_ephemeral_and_cannot_escalate_to_lineage() {
        let mut fixture = fixture();
        fixture
            .resolver
            .provision_scratch(&fixture.authority, &fixture.access, &fixture.scratch, 4)
            .unwrap();
        let bytes = b"12345".to_vec();
        let digest = digest_bytes(&bytes);
        assert!(fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.scratch,
                &digest,
                bytes,
            )
            .is_err());
        let bytes = b"1234".to_vec();
        let digest = digest_bytes(&bytes);
        fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.scratch,
                &digest,
                bytes,
            )
            .unwrap();
        assert!(fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.scratch,
                &digest_bytes(b"abcd"),
                b"abcd".to_vec(),
            )
            .is_err());
        let req = request(
            &fixture,
            0,
            ResourceVerbV1::Create,
            fixture.scratch.clone(),
            "scratch.txt",
            Some(digest),
            vec![],
            request_budget(0, 4),
        );
        let evidence = enforce(&mut fixture, &req);
        assert_eq!(evidence.decision, EffectDecisionV1::Allowed);
        assert!(fixture
            .resolver
            .seal_output_slot(
                &fixture.authority,
                &fixture.access,
                &fixture.scratch,
                "scratch.txt",
                &evidence,
            )
            .is_err());
    }

    #[test]
    fn alias_and_changed_private_identity_are_rejected_where_supported() {
        let mut fixture = fixture();
        fixture
            .resolver
            .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
            .unwrap();
        let first = b"first".to_vec();
        let first_digest = digest_bytes(&first);
        fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.output,
                &first_digest,
                first,
            )
            .unwrap();
        let create = request(
            &fixture,
            0,
            ResourceVerbV1::Create,
            fixture.output.clone(),
            "result.txt",
            Some(first_digest.clone()),
            vec![],
            request_budget(0, 16),
        );
        assert_eq!(
            enforce(&mut fixture, &create).decision,
            EffectDecisionV1::Allowed
        );
        let path = match fixture.resolver.backings.get(&fixture.output).unwrap() {
            HostResourceBackingV1::OutputSlot { files, .. } => files["result.txt"].path.clone(),
            _ => unreachable!(),
        };
        #[cfg(any(unix, windows))]
        fs::hard_link(&path, fixture.root.join("alias.txt")).unwrap();
        let second = b"second".to_vec();
        let second_digest = digest_bytes(&second);
        fixture
            .resolver
            .stage_write_payload(
                &fixture.authority,
                &fixture.access,
                &fixture.output,
                &second_digest,
                second,
            )
            .unwrap();
        let replace = request(
            &fixture,
            1,
            ResourceVerbV1::Replace,
            fixture.output.clone(),
            "result.txt",
            Some(second_digest),
            vec![EffectPreconditionV1::ResourceGeneration {
                handle_ref: fixture.output.clone(),
                generation: 1,
                digest: first_digest,
            }],
            request_budget(0, 16),
        );
        assert_eq!(
            enforce(&mut fixture, &replace).decision,
            EffectDecisionV1::Denied
        );
    }

    #[test]
    fn cancellation_disconnect_burn_and_restart_invalidate_resolution() {
        for lifecycle in [
            "cancel",
            "revoke",
            "disconnect",
            "expiry",
            "burn",
            "restart",
        ] {
            let mut fixture = fixture();
            fixture
                .resolver
                .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
                .unwrap();
            match lifecycle {
                "cancel" => fixture
                    .authority
                    .cancel_run(&fixture.access.run_control_ref)
                    .unwrap(),
                "revoke" => fixture
                    .authority
                    .revoke_run(&fixture.access.run_control_ref)
                    .unwrap(),
                "disconnect" => fixture.access.current.disconnected = true,
                "expiry" => fixture.access.current.now = fixture.access.context.expires_at,
                "burn" => fixture.access.current.burned = true,
                "restart" => fixture.access.current.restarted = true,
                _ => unreachable!(),
            }
            assert!(fixture
                .resolver
                .stage_write_payload(
                    &fixture.authority,
                    &fixture.access,
                    &fixture.output,
                    &digest_bytes(b"blocked"),
                    b"blocked".to_vec(),
                )
                .is_err());
        }
    }

    #[test]
    fn managed_revision_mutation_grants_are_structurally_rejected() {
        let mut fixture = fixture();
        let draft = fixture
            .authority
            .begin_run(fixture.access.context.clone())
            .unwrap();
        assert!(fixture
            .authority
            .mint_resource_grant(
                &draft,
                ResourceGrantSpecV1 {
                    host_ref: fixture.access.context.host_ref.clone(),
                    kind: ResourceKindV1::ManagedRevision,
                    safe_identity_ref: "forged-mutable-managed-revision".into(),
                    selector_prefix: ".".into(),
                    allowed_verbs: [ResourceVerbV1::Replace].into_iter().collect(),
                    budgets: total_budget(),
                    expires_at: fixture.access.context.expires_at,
                },
            )
            .is_err());
    }

    fn provision_process_world(
        fixture: &mut Fixture,
        worlds: &ExecutionWorldServiceV1,
    ) -> AppResult<()> {
        fixture.resolver.bind_managed_revision(
            &fixture.authority,
            &mut fixture.objects,
            &fixture.access,
            &fixture.managed,
            fixture.acquisition.clone(),
        )?;
        fixture.resolver.bind_workspace(
            &fixture.authority,
            &mut fixture.objects,
            &fixture.access,
            &fixture.workspace,
            WorkspaceBindingSpecV1 {
                acquisition: fixture.acquisition.clone(),
                initial_selector: "project/input.txt".into(),
                quota_bytes: 16 * 1024,
            },
        )?;
        fixture.resolver.provision_output_slot(
            &fixture.authority,
            &fixture.access,
            &fixture.output,
            16 * 1024,
        )?;
        fixture.resolver.provision_scratch(
            &fixture.authority,
            &fixture.access,
            &fixture.scratch,
            16 * 1024,
        )?;
        worlds.provision_world(
            &fixture.authority,
            &mut fixture.resolver,
            &mut fixture.objects,
            fixture.access.clone(),
            &fixture.world,
        )
    }

    fn process_budget(read_bytes: u64, wall_millis: u64) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            read_bytes,
            write_bytes: 16 * 1024,
            process_spawns: 1,
            process_signals: 1,
            cpu_millis: 2_000,
            memory_byte_millis: 2 * 1024 * 1024 * 1024 * wall_millis,
            wall_millis,
            ..EffectBudgetsV1::default()
        }
    }

    fn signal_budget(read_bytes: u64) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            read_bytes,
            process_signals: 1,
            ..EffectBudgetsV1::default()
        }
    }

    fn process_fixture() -> Fixture {
        fixture()
    }

    fn process_request(
        fixture: &Fixture,
        sequence: u64,
        effect: ProcessEffectV1,
        budget: EffectBudgetsV1,
    ) -> EffectRequestV1 {
        lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: fixture.access.context.clone(),
                envelope_ref: fixture.access.envelope_ref.clone(),
                run_control_ref: fixture.access.run_control_ref.clone(),
                first_sequence: sequence,
            },
            &ToolRequestV1 {
                tool_name: "synthetic-process-conformance-tool".into(),
                adapter_version_ref: "synthetic-process-conformance-adapter-v1".into(),
                intents: vec![ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Process(effect),
                    requested_budget_slice: budget,
                    preconditions: vec![],
                }],
            },
        )
        .unwrap()
        .remove(0)
    }

    fn spawn_effect(fixture: &Fixture, invocation: &ManagedProcessInvocationV1) -> ProcessEffectV1 {
        ProcessEffectV1::Spawn {
            world_ref: fixture.world.clone(),
            executable_handle: invocation.executable_handle.clone(),
            argv_digest: invocation.argv_digest().unwrap(),
            working_directory_handle: invocation.working_directory_handle.clone(),
            working_directory_selector: invocation.working_directory_selector.clone(),
            environment_digest: invocation.environment_digest().unwrap(),
            stdin_digest: invocation.stdin_digest().unwrap(),
        }
    }

    #[test]
    fn execution_world_conformance_is_complete_or_unavailable() {
        let availability = ExecutionWorldServiceV1::platform_availability();
        let required = [
            ConfinementPropertyV1::NoAmbientFilesystem,
            ConfinementPropertyV1::EmptyEnvironment,
            ConfinementPropertyV1::NoInheritedDescriptors,
            ConfinementPropertyV1::ContainedProcessTree,
            ConfinementPropertyV1::NoDaemonSurvival,
            ConfinementPropertyV1::NoRawNetwork,
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if availability.available {
            assert!(availability.verified_properties.is_superset(&required));
            assert!(availability.unavailable_reason.is_none());
        } else {
            assert!(availability.verified_properties.is_empty());
            assert!(availability.unavailable_reason.is_some());
        }
    }

    #[test]
    fn world_context_substitution_and_dangerous_environment_are_rejected() {
        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        if ExecutionWorldServiceV1::platform_availability().available {
            provision_process_world(&mut fixture, &worlds).unwrap();
            let mut wrong = fixture.access.clone();
            wrong.context.attempt_id = "substituted-attempt".into();
            let invocation = ManagedProcessInvocationV1 {
                executable_handle: fixture.executable.clone(),
                argv: vec!["-c".into(), "exit 0".into()],
                environment: BTreeMap::new(),
                stdin: None,
                working_directory_handle: None,
                working_directory_selector: None,
            };
            assert!(worlds
                .stage_invocation(&wrong, &fixture.world, invocation)
                .is_err());
            let wrong_world: ExecutionWorldRefV1 =
                serde_json::from_value(serde_json::json!("substituted-world")).unwrap();
            let substituted_resource = ManagedProcessInvocationV1 {
                executable_handle: fixture.workspace.clone(),
                argv: vec!["-c".into(), "exit 0".into()],
                environment: BTreeMap::new(),
                stdin: None,
                working_directory_handle: None,
                working_directory_selector: None,
            };
            assert!(worlds
                .stage_invocation(&fixture.access, &wrong_world, substituted_resource.clone())
                .is_err());
            assert!(worlds
                .stage_invocation(&fixture.access, &fixture.world, substituted_resource)
                .is_err());
        }

        for name in [
            "HOME",
            "PATH",
            "USERPROFILE",
            "TEMP",
            "APPDATA",
            "SYSTEMROOT",
            "SSH_AUTH_SOCK",
            "AWS_SECRET_ACCESS_KEY",
            "DYLD_INSERT_LIBRARIES",
        ] {
            let invocation = ManagedProcessInvocationV1 {
                executable_handle: fixture.executable.clone(),
                argv: vec![],
                environment: [(name.into(), "ambient-secret".into())]
                    .into_iter()
                    .collect(),
                stdin: None,
                working_directory_handle: None,
                working_directory_selector: None,
            };
            assert!(super::super::execution_world::validate_invocation(&invocation).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_world_denies_ambient_host_and_network_and_emits_ordered_process_evidence() {
        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        if !ExecutionWorldServiceV1::platform_availability().available {
            assert!(provision_process_world(&mut fixture, &worlds).is_err());
            return;
        }
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec![
                "-c".into(),
                "test -z \"$HOME\"; test ! -r /etc/passwd; if /usr/bin/curl -s --max-time 1 https://127.0.0.1 >/dev/null 2>&1; then exit 19; fi; if (sleep 1 &) 2>/dev/null; then exit 18; fi; i=3; while [ \"$i\" -lt 40 ]; do eval \"exec $i>fd-$i\" || break; i=$((i+1)); done; test \"$i\" -lt 40; printf generated > result.txt; printf contained"
                    .into(),
            ],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.output.clone()),
            working_directory_selector: Some(".".into()),
        };
        let digests = worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            process_budget(4096, 2_000),
        );
        let evidence = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&spawn, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert_eq!(evidence.decision, EffectDecisionV1::Allowed);
        let process_ref = match &evidence.facts {
            EffectFactsV1::ContainedProcess {
                process_ref,
                argv_digest,
                environment_digest,
                network_denied,
                ..
            } => {
                assert_eq!(argv_digest, &digests.0);
                assert_eq!(environment_digest, &digests.1);
                assert!(*network_denied);
                process_ref.clone()
            }
            other => panic!("unexpected process facts: {other:?}"),
        };
        let signal = process_request(
            &fixture,
            1,
            ProcessEffectV1::Signal {
                world_ref: fixture.world.clone(),
                process_ref,
                signal_ref: "terminate".into(),
            },
            signal_budget(4096),
        );
        std::thread::sleep(Duration::from_millis(75));
        let terminal = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&signal, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert_eq!(terminal.decision, EffectDecisionV1::Allowed);
        assert_eq!(
            terminal.prior_evidence_digest,
            Some(evidence.evidence_digest)
        );
        assert!(
            matches!(
                terminal.facts,
                EffectFactsV1::ContainedProcess {
                    descendants_terminated: true,
                    network_denied: true,
                    exit_code: Some(0),
                    ref state,
                    ..
                } if state == "exited"
            ),
            "terminal evidence: {terminal:?}"
        );
        let generated = match fixture.resolver.backings.get(&fixture.output).unwrap() {
            HostResourceBackingV1::OutputSlot { files, .. } => {
                files.get("result.txt").expect("contained output committed")
            }
            _ => unreachable!(),
        };
        assert_eq!(generated.generation, 1);
        assert_eq!(generated.identity.digest, digest_bytes(b"generated"));
        assert_eq!(
            fs::read(&fixture.source_path).unwrap(),
            b"authoritative revision N"
        );
        fixture
            .authority
            .validate_evidence_chain(&fixture.access.run_control_ref)
            .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn output_budget_external_signal_and_burn_cleanup_fail_closed() {
        let mut fixture = fixture();
        let worlds = ExecutionWorldServiceV1::default();
        if !ExecutionWorldServiceV1::platform_availability().available {
            assert!(provision_process_world(&mut fixture, &worlds).is_err());
            return;
        }
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec!["-c".into(), "while :; do printf 0123456789; done".into()],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.scratch.clone()),
            working_directory_selector: Some(".".into()),
        };
        worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            process_budget(64, 2_000),
        );
        let evidence = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&spawn, &fixture.access.current, &mut backend)
                .unwrap()
        };
        let process_ref = match evidence.facts {
            EffectFactsV1::ContainedProcess { process_ref, .. } => process_ref,
            _ => panic!("spawn did not produce contained process evidence"),
        };

        let outside = process_request(
            &fixture,
            1,
            ProcessEffectV1::Signal {
                world_ref: fixture.world.clone(),
                process_ref: "unowned-host-pid-1".into(),
                signal_ref: "kill".into(),
            },
            signal_budget(64),
        );
        let denial = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&outside, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert_eq!(denial.decision, EffectDecisionV1::Denied);
        std::thread::sleep(Duration::from_millis(75));
        let exact = process_request(
            &fixture,
            2,
            ProcessEffectV1::Signal {
                world_ref: fixture.world.clone(),
                process_ref: process_ref.clone(),
                signal_ref: "terminate".into(),
            },
            signal_budget(64),
        );
        let terminal = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&exact, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert!(
            matches!(
                terminal.facts,
                EffectFactsV1::ContainedProcess {
                    ref state,
                    stdout_bytes: 32,
                    descendants_terminated: true,
                    ..
                } if state == "output_budget_exceeded"
            ),
            "terminal evidence: {terminal:?}"
        );
        assert_eq!(worlds.terminate_bridge("bridge-step5"), 0);

        // A fresh run proves explicit lifecycle termination is reliable even
        // without a signal EffectRequest; this is the path Burn, disconnect,
        // cancellation, expiry coordination, and shutdown call.
        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec!["-c".into(), "while :; do :; done".into()],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.scratch.clone()),
            working_directory_selector: Some(".".into()),
        };
        worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            process_budget(64, 2_000),
        );
        {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            assert_eq!(
                fixture
                    .authority
                    .enforce(&spawn, &fixture.access.current, &mut backend)
                    .unwrap()
                    .decision,
                EffectDecisionV1::Allowed
            );
        }
        assert_eq!(worlds.terminate_bridge("bridge-step5"), 1);
        assert_eq!(worlds.terminate_bridge("bridge-step5"), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wall_expiry_and_run_cancellation_terminate_the_contained_tree() {
        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        if !ExecutionWorldServiceV1::platform_availability().available {
            assert!(provision_process_world(&mut fixture, &worlds).is_err());
            return;
        }
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec!["-c".into(), "while :; do :; done".into()],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.scratch.clone()),
            working_directory_selector: Some(".".into()),
        };
        worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            process_budget(4096, 40),
        );
        let process_ref = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            match fixture
                .authority
                .enforce(&spawn, &fixture.access.current, &mut backend)
                .unwrap()
                .facts
            {
                EffectFactsV1::ContainedProcess { process_ref, .. } => process_ref,
                _ => panic!("wall-time test did not start a contained process"),
            }
        };
        std::thread::sleep(Duration::from_millis(100));
        let signal = process_request(
            &fixture,
            1,
            ProcessEffectV1::Signal {
                world_ref: fixture.world.clone(),
                process_ref,
                signal_ref: "terminate".into(),
            },
            signal_budget(4096),
        );
        let terminal = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&signal, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert!(matches!(
            terminal.facts,
            EffectFactsV1::ContainedProcess {
                ref state,
                descendants_terminated: true,
                ..
            } if state == "wall_time_expired"
        ));

        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec!["-c".into(), "while :; do :; done".into()],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.scratch.clone()),
            working_directory_selector: Some(".".into()),
        };
        worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            process_budget(4096, 2_000),
        );
        {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            assert_eq!(
                fixture
                    .authority
                    .enforce(&spawn, &fixture.access.current, &mut backend)
                    .unwrap()
                    .decision,
                EffectDecisionV1::Allowed
            );
        }
        assert_eq!(worlds.terminate_run(&fixture.access.run_control_ref), 1);
        assert_eq!(worlds.terminate_run(&fixture.access.run_control_ref), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn contained_process_write_budget_is_hard_bounded() {
        let mut fixture = process_fixture();
        let worlds = ExecutionWorldServiceV1::default();
        if !ExecutionWorldServiceV1::platform_availability().available {
            assert!(provision_process_world(&mut fixture, &worlds).is_err());
            return;
        }
        provision_process_world(&mut fixture, &worlds).unwrap();
        let invocation = ManagedProcessInvocationV1 {
            executable_handle: fixture.executable.clone(),
            argv: vec![
                "-c".into(),
                "while :; do printf 0123456789 >> bounded.txt; done".into(),
            ],
            environment: BTreeMap::new(),
            stdin: None,
            working_directory_handle: Some(fixture.scratch.clone()),
            working_directory_selector: Some(".".into()),
        };
        worlds
            .stage_invocation(&fixture.access, &fixture.world, invocation.clone())
            .unwrap();
        let mut spawn_budget = process_budget(4096, 2_000);
        spawn_budget.write_bytes = 64;
        let spawn = process_request(
            &fixture,
            0,
            spawn_effect(&fixture, &invocation),
            spawn_budget,
        );
        let process_ref = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            match fixture
                .authority
                .enforce(&spawn, &fixture.access.current, &mut backend)
                .unwrap()
                .facts
            {
                EffectFactsV1::ContainedProcess { process_ref, .. } => process_ref,
                _ => panic!("write-budget test did not start a contained process"),
            }
        };
        std::thread::sleep(Duration::from_millis(75));
        let signal = process_request(
            &fixture,
            1,
            ProcessEffectV1::Signal {
                world_ref: fixture.world.clone(),
                process_ref,
                signal_ref: "terminate".into(),
            },
            signal_budget(4096),
        );
        let terminal = {
            let mut backend = HostManagedProcessBackendV1::new(
                &worlds,
                &mut fixture.resolver,
                &mut fixture.objects,
            );
            fixture
                .authority
                .enforce(&signal, &fixture.access.current, &mut backend)
                .unwrap()
        };
        assert!(matches!(
            terminal.facts,
            EffectFactsV1::ContainedProcess { ref state, .. }
                if state == "resource_budget_exceeded"
        ));
        let bytes = match fixture.resolver.backings.get(&fixture.scratch).unwrap() {
            HostResourceBackingV1::Scratch { files, .. } => files
                .get("bounded.txt")
                .map_or(0, |file| file.identity.byte_count),
            _ => unreachable!(),
        };
        assert!(bytes <= 64);
    }

    #[cfg(unix)]
    #[test]
    fn writable_overlay_rejects_symlink_and_never_mutates_managed_revision() {
        use std::os::unix::fs::symlink;

        let mut fixture = fixture();
        fixture
            .resolver
            .provision_output_slot(&fixture.authority, &fixture.access, &fixture.output, 4096)
            .unwrap();
        let grant = fixture
            .authority
            .validate_resource_attachment(
                &fixture.output,
                ResourceKindV1::OutputSlot,
                &fixture.access.envelope_ref,
                &fixture.access.run_control_ref,
                &fixture.access.context,
                &fixture.access.current,
            )
            .unwrap();
        let mounts = fixture
            .resolver
            .lease_execution_world_mounts(
                &fixture.authority,
                &mut fixture.objects,
                &fixture.access,
                &[grant],
            )
            .unwrap();
        assert!(mounts[0].private_overlay);
        symlink(&fixture.source_path, mounts[0].source_path.join("escape")).unwrap();
        let request_id: EffectRequestIdV1 =
            serde_json::from_value(serde_json::json!("pastey-test-process-request")).unwrap();
        assert!(fixture
            .resolver
            .release_execution_world_mounts(
                &mut fixture.objects,
                &fixture.access,
                &request_id,
                &mounts,
            )
            .is_err());
        assert_eq!(
            fs::read(&fixture.source_path).unwrap(),
            b"authoritative revision N"
        );
    }
}
