//! Minimal extraction of the Codex product permission types used by the
//! Windows sandbox mechanics.
//!
//! These types are private to the derived crate. Pastey supplies concrete
//! roots through the mechanics API and never treats this policy shape as
//! authority.

use codex_utils_absolute_path::AbsolutePathBuf;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PROJECT_ROOTS_GLOB_PATTERN_PREFIX: &str = "codex-project-roots://";

pub fn project_roots_glob_pattern(subpath: &Path) -> String {
    format!("{PROJECT_ROOTS_GLOB_PATTERN_PREFIX}{}", subpath.display())
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowsSandboxLevel {
    #[default]
    Disabled,
    RestrictedToken,
    Elevated,
}

impl std::fmt::Display for WindowsSandboxLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disabled => "disabled",
            Self::RestrictedToken => "restricted-token",
            Self::Elevated => "elevated",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkSandboxPolicy {
    #[default]
    Restricted,
    Enabled,
}

impl NetworkSandboxPolicy {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileSystemAccessMode {
    Read,
    Write,
    #[serde(alias = "none")]
    Deny,
}

impl FileSystemAccessMode {
    pub fn can_read(self) -> bool {
        !matches!(self, Self::Deny)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Write)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSystemSpecialPath {
    Root,
    Minimal,
    ProjectRoots {
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    Unknown {
        path: String,
        subpath: Option<String>,
    },
}

impl FileSystemSpecialPath {
    pub fn project_roots(subpath: Option<String>) -> Self {
        Self::ProjectRoots { subpath }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemPath {
    Path { path: AbsolutePathBuf },
    GlobPattern { pattern: String },
    Special { value: FileSystemSpecialPath },
}

impl From<AbsolutePathBuf> for FileSystemPath {
    fn from(path: AbsolutePathBuf) -> Self {
        Self::Path { path }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystemSandboxEntryMissingPathBehavior {
    Skip,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct FileSystemSandboxEntry {
    pub path: FileSystemPath,
    pub access: FileSystemAccessMode,
    #[serde(default)]
    pub missing_path_behavior: Option<FileSystemSandboxEntryMissingPathBehavior>,
}

impl FileSystemSandboxEntry {
    pub fn new(path: FileSystemPath, access: FileSystemAccessMode) -> Self {
        Self {
            path,
            access,
            missing_path_behavior: None,
        }
    }

    fn skips_missing_path(&self) -> bool {
        self.missing_path_behavior == Some(FileSystemSandboxEntryMissingPathBehavior::Skip)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileSystemSandboxKind {
    #[default]
    Restricted,
    Unrestricted,
    ExternalSandbox,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileSystemSandboxPolicy {
    pub kind: FileSystemSandboxKind,
    pub glob_scan_max_depth: Option<usize>,
    pub entries: Vec<FileSystemSandboxEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WritableRoot {
    pub root: AbsolutePathBuf,
    pub read_only_subpaths: Vec<AbsolutePathBuf>,
}

impl FileSystemSandboxPolicy {
    pub fn restricted(entries: Vec<FileSystemSandboxEntry>) -> Self {
        Self {
            kind: FileSystemSandboxKind::Restricted,
            glob_scan_max_depth: None,
            entries,
        }
    }

    pub fn read_only() -> Self {
        Self::restricted(vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        )])
    }

    pub fn unrestricted() -> Self {
        Self {
            kind: FileSystemSandboxKind::Unrestricted,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn external_sandbox() -> Self {
        Self {
            kind: FileSystemSandboxKind::ExternalSandbox,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn workspace_write(
        writable_roots: &[AbsolutePathBuf],
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    ) -> Self {
        let mut entries = vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::project_roots(None),
            },
            FileSystemAccessMode::Write,
        )];
        entries.extend(
            writable_roots
                .iter()
                .cloned()
                .map(|path| FileSystemSandboxEntry::new(path.into(), FileSystemAccessMode::Write)),
        );
        if !exclude_tmpdir_env_var {
            entries.push(FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::Tmpdir,
                },
                FileSystemAccessMode::Write,
            ));
        }
        if !exclude_slash_tmp {
            entries.push(FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::SlashTmp,
                },
                FileSystemAccessMode::Write,
            ));
        }
        Self::restricted(entries)
    }

    pub fn materialize_project_roots_with_workspace_roots(
        self,
        workspace_roots: &[AbsolutePathBuf],
    ) -> Self {
        let mut entries = Vec::new();
        for entry in self.entries {
            match entry.path {
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::ProjectRoots { subpath },
                } => {
                    entries.extend(workspace_roots.iter().filter_map(|root| {
                        let path = subpath.as_deref().map_or_else(
                            || root.as_path().to_path_buf(),
                            |subpath| root.as_path().join(subpath),
                        );
                        AbsolutePathBuf::from_absolute_path(path).ok().map(|path| {
                            FileSystemSandboxEntry {
                                path: path.into(),
                                access: entry.access,
                                missing_path_behavior: entry.missing_path_behavior,
                            }
                        })
                    }));
                }
                path => entries.push(FileSystemSandboxEntry { path, ..entry }),
            }
        }
        Self { entries, ..self }
    }

    pub fn remove_skip_missing_path_entries(&mut self) {
        self.entries.retain(|entry| {
            !entry.skips_missing_path()
                || entry_path(entry, Path::new("C:\\")).is_some_and(|path| path.exists())
        });
    }

    pub fn has_full_disk_read_access(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Unrestricted)
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.path,
                    FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root
                    }
                ) && entry.access.can_read()
            })
    }

    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Unrestricted)
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.path,
                    FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root
                    }
                ) && entry.access.can_write()
            })
    }

    pub fn include_platform_defaults(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
    }

    pub fn can_read_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        self.matching_access(path, cwd)
            .is_some_and(FileSystemAccessMode::can_read)
    }

    pub fn get_readable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        self.entries
            .iter()
            .filter(|entry| entry.access.can_read())
            .filter_map(|entry| {
                entry_path(entry, cwd)
                    .and_then(|path| AbsolutePathBuf::from_absolute_path(path).ok())
            })
            .collect()
    }

    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        self.entries
            .iter()
            .filter(|entry| entry.access.can_write())
            .filter_map(|entry| {
                let root = entry_path(entry, cwd)
                    .and_then(|path| AbsolutePathBuf::from_absolute_path(path).ok())?;
                let read_only_subpaths = self
                    .entries
                    .iter()
                    .filter(|candidate| !candidate.access.can_write())
                    .filter_map(|candidate| {
                        let path = entry_path(candidate, cwd)?;
                        path.starts_with(root.as_path())
                            .then(|| AbsolutePathBuf::from_absolute_path(path).ok())
                            .flatten()
                    })
                    .collect();
                Some(WritableRoot {
                    root,
                    read_only_subpaths,
                })
            })
            .collect()
    }

    pub fn get_unreadable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        self.entries
            .iter()
            .filter(|entry| entry.access == FileSystemAccessMode::Deny)
            .filter_map(|entry| {
                entry_path(entry, cwd)
                    .and_then(|path| AbsolutePathBuf::from_absolute_path(path).ok())
            })
            .collect()
    }

    pub fn get_unreadable_globs_with_cwd(&self, _cwd: &Path) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|entry| match (&entry.path, entry.access) {
                (FileSystemPath::GlobPattern { pattern }, FileSystemAccessMode::Deny) => {
                    Some(pattern.clone())
                }
                _ => None,
            })
            .collect()
    }

    pub fn has_explicit_non_write_entry_for_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        self.entries
            .iter()
            .filter(|entry| !entry.access.can_write())
            .any(|entry| entry_path(entry, cwd).as_deref() == Some(path))
    }

    fn matching_access(&self, path: &Path, cwd: &Path) -> Option<FileSystemAccessMode> {
        if matches!(self.kind, FileSystemSandboxKind::Unrestricted) {
            return Some(FileSystemAccessMode::Write);
        }
        self.entries
            .iter()
            .filter_map(|entry| {
                let root = entry_path(entry, cwd)?;
                path.starts_with(&root)
                    .then_some((root.components().count(), entry.access))
            })
            .max_by_key(|(specificity, access)| (*specificity, *access))
            .map(|(_, access)| access)
    }
}

fn entry_path(entry: &FileSystemSandboxEntry, cwd: &Path) -> Option<PathBuf> {
    match &entry.path {
        FileSystemPath::Path { path } => Some(path.as_path().to_path_buf()),
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        } => cwd.ancestors().last().map(Path::to_path_buf),
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Minimal,
        } => None,
        FileSystemPath::Special {
            value: FileSystemSpecialPath::ProjectRoots { subpath },
        } => Some(
            subpath
                .as_deref()
                .map_or_else(|| cwd.to_path_buf(), |subpath| cwd.join(subpath)),
        ),
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Tmpdir,
        } => std::env::temp_dir().into(),
        FileSystemPath::Special {
            value: FileSystemSpecialPath::SlashTmp,
        } => None,
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Unknown { .. },
        }
        | FileSystemPath::GlobPattern { .. } => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadDenyMatcher {
    exact_roots: Vec<PathBuf>,
    globs: Vec<glob::Pattern>,
}

impl ReadDenyMatcher {
    pub fn try_new(policy: &FileSystemSandboxPolicy, cwd: &Path) -> Result<Option<Self>, String> {
        let exact_roots = policy
            .get_unreadable_roots_with_cwd(cwd)
            .into_iter()
            .map(AbsolutePathBuf::into_path_buf)
            .collect::<Vec<_>>();
        let globs = policy
            .get_unreadable_globs_with_cwd(cwd)
            .into_iter()
            .map(|pattern| glob::Pattern::new(&pattern).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        if exact_roots.is_empty() && globs.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Self { exact_roots, globs }))
        }
    }

    pub fn is_read_denied(&self, path: &Path) -> bool {
        self.exact_roots.iter().any(|root| path.starts_with(root))
            || self.globs.iter().any(|pattern| pattern.matches_path(path))
    }

    pub fn is_read_denied_with_canonical_path(&self, path: &Path, canonical: &Path) -> bool {
        self.is_read_denied(path) || self.is_read_denied(canonical)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedFileSystemPermissions {
    Restricted {
        entries: Vec<FileSystemSandboxEntry>,
        glob_scan_max_depth: Option<usize>,
    },
    Unrestricted,
}

impl ManagedFileSystemPermissions {
    fn from_sandbox_policy(policy: &FileSystemSandboxPolicy) -> Self {
        match policy.kind {
            FileSystemSandboxKind::Restricted => Self::Restricted {
                entries: policy.entries.clone(),
                glob_scan_max_depth: policy.glob_scan_max_depth,
            },
            FileSystemSandboxKind::Unrestricted => Self::Unrestricted,
            FileSystemSandboxKind::ExternalSandbox => {
                unreachable!("external sandbox is represented separately")
            }
        }
    }

    fn to_sandbox_policy(&self) -> FileSystemSandboxPolicy {
        match self {
            Self::Restricted {
                entries,
                glob_scan_max_depth,
            } => FileSystemSandboxPolicy {
                kind: FileSystemSandboxKind::Restricted,
                entries: entries.clone(),
                glob_scan_max_depth: *glob_scan_max_depth,
            },
            Self::Unrestricted => FileSystemSandboxPolicy::unrestricted(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionProfile {
    Managed {
        file_system: ManagedFileSystemPermissions,
        network: NetworkSandboxPolicy,
    },
    Disabled,
    External {
        network: NetworkSandboxPolicy,
    },
}

impl Default for PermissionProfile {
    fn default() -> Self {
        Self::read_only()
    }
}

impl PermissionProfile {
    pub fn read_only() -> Self {
        Self::from_runtime_permissions(
            &FileSystemSandboxPolicy::read_only(),
            NetworkSandboxPolicy::Restricted,
        )
    }

    pub fn workspace_write() -> Self {
        Self::workspace_write_with(&[], NetworkSandboxPolicy::Restricted, false, false)
    }

    pub fn workspace_write_with(
        writable_roots: &[AbsolutePathBuf],
        network: NetworkSandboxPolicy,
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    ) -> Self {
        Self::from_runtime_permissions(
            &FileSystemSandboxPolicy::workspace_write(
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
            ),
            network,
        )
    }

    pub fn from_runtime_permissions(
        policy: &FileSystemSandboxPolicy,
        network: NetworkSandboxPolicy,
    ) -> Self {
        match policy.kind {
            FileSystemSandboxKind::Restricted | FileSystemSandboxKind::Unrestricted => {
                Self::Managed {
                    file_system: ManagedFileSystemPermissions::from_sandbox_policy(policy),
                    network,
                }
            }
            FileSystemSandboxKind::ExternalSandbox => Self::External { network },
        }
    }

    pub fn file_system_sandbox_policy(&self) -> FileSystemSandboxPolicy {
        match self {
            Self::Managed { file_system, .. } => file_system.to_sandbox_policy(),
            Self::Disabled => FileSystemSandboxPolicy::unrestricted(),
            Self::External { .. } => FileSystemSandboxPolicy::external_sandbox(),
        }
    }

    pub fn to_runtime_permissions(&self) -> (FileSystemSandboxPolicy, NetworkSandboxPolicy) {
        let network = match self {
            Self::Managed { network, .. } | Self::External { network } => *network,
            Self::Disabled => NetworkSandboxPolicy::Enabled,
        };
        (self.file_system_sandbox_policy(), network)
    }
}
