//! Reusable, mechanics-only entry points over the upstream Windows sandbox.

use crate::models::{ManagedFileSystemPermissions, PermissionProfile};
use crate::permissions::{
    FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, NetworkSandboxPolicy,
};
use crate::{
    WindowsSandboxProvisioningSettings, WindowsSandboxProxySettingsMode,
    WindowsSandboxSessionRequest, config_types::WindowsSandboxLevel,
    run_elevated_provisioning_setup, sandbox_setup_is_complete,
    spawn_windows_sandbox_session_for_level,
};
use anyhow::{Context, Result};
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_pty::SpawnedProcess;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Concrete Windows process launch data. Callers must resolve authorization
/// before constructing this value.
pub struct WindowsSandboxLaunch {
    pub sandbox_home: PathBuf,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub environment: HashMap<String, String>,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub stdin_open: bool,
}

/// Returns the coarse upstream setup state. Pastey additionally applies its
/// native conformance probe before advertising backend availability.
pub fn setup_is_complete(sandbox_home: &Path) -> bool {
    sandbox_setup_is_complete(sandbox_home)
}

/// Runs the upstream elevated provisioning transaction. This entry point is
/// intended only for an explicit Host-owned setup command.
pub fn run_host_setup(sandbox_home: &Path, host_username: &str) -> Result<()> {
    run_elevated_provisioning_setup(
        sandbox_home,
        host_username,
        WindowsSandboxProvisioningSettings::default(),
    )
}

pub async fn spawn(launch: WindowsSandboxLaunch) -> Result<SpawnedProcess> {
    if !setup_is_complete(&launch.sandbox_home) {
        anyhow::bail!("Windows sandbox setup is missing or out of date");
    }
    if launch.command.is_empty() {
        anyhow::bail!("Windows sandbox command is empty");
    }

    let read_roots = absolute_roots(&launch.read_roots).context("validate readable roots")?;
    let write_roots = absolute_roots(&launch.write_roots).context("validate writable roots")?;
    let permission_profile = mechanics_profile(&read_roots, &write_roots);
    let mut workspace_roots = write_roots.clone();
    if workspace_roots.is_empty() {
        workspace_roots.push(
            AbsolutePathBuf::from_absolute_path(&launch.cwd)
                .context("validate command working directory")?,
        );
    }

    spawn_windows_sandbox_session_for_level(WindowsSandboxSessionRequest {
        permission_profile: &permission_profile,
        workspace_roots: &workspace_roots,
        codex_home: &launch.sandbox_home,
        command: launch.command,
        cwd: &launch.cwd,
        env_map: launch.environment,
        windows_sandbox_level: WindowsSandboxLevel::Elevated,
        proxy_enforced: false,
        network_proxy_restricting_sid: None,
        proxy_settings_mode: WindowsSandboxProxySettingsMode::Preserve,
        timeout_ms: None,
        read_roots_override: Some(&launch.read_roots),
        read_roots_include_platform_defaults: true,
        write_roots_override: Some(&launch.write_roots),
        deny_read_paths_override: &[],
        deny_write_paths_override: &[],
        tty: false,
        stdin_open: launch.stdin_open,
        use_private_desktop: true,
    })
    .await
}

fn absolute_roots(roots: &[PathBuf]) -> Result<Vec<AbsolutePathBuf>> {
    roots
        .iter()
        .map(AbsolutePathBuf::from_absolute_path)
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::msg)
}

fn mechanics_profile(
    read_roots: &[AbsolutePathBuf],
    write_roots: &[AbsolutePathBuf],
) -> PermissionProfile {
    let mut entries = read_roots
        .iter()
        .cloned()
        .map(|path| {
            FileSystemSandboxEntry::new(FileSystemPath::from(path), FileSystemAccessMode::Read)
        })
        .collect::<Vec<_>>();
    entries.extend(write_roots.iter().cloned().map(|path| {
        FileSystemSandboxEntry::new(FileSystemPath::from(path), FileSystemAccessMode::Write)
    }));
    PermissionProfile::Managed {
        file_system: ManagedFileSystemPermissions::Restricted {
            entries,
            glob_scan_max_depth: None,
        },
        network: NetworkSandboxPolicy::Restricted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mechanics_profile_is_always_network_restricted() {
        let root = AbsolutePathBuf::from_absolute_path(if cfg!(windows) {
            PathBuf::from(r"C:\workspace")
        } else {
            PathBuf::from("/workspace")
        })
        .expect("absolute root");
        let profile = mechanics_profile(std::slice::from_ref(&root), std::slice::from_ref(&root));
        assert!(matches!(
            profile,
            PermissionProfile::Managed {
                network: NetworkSandboxPolicy::Restricted,
                ..
            }
        ));
    }

    #[test]
    fn mechanics_profile_contains_only_explicit_caller_roots() {
        let read = AbsolutePathBuf::from_absolute_path(PathBuf::from(r"C:\authorized-read"))
            .expect("absolute read root");
        let write = AbsolutePathBuf::from_absolute_path(PathBuf::from(r"C:\authorized-write"))
            .expect("absolute write root");
        let profile = mechanics_profile(std::slice::from_ref(&read), std::slice::from_ref(&write));
        let PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Restricted { entries, .. },
            ..
        } = profile
        else {
            panic!("mechanics profile must remain explicitly restricted");
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| {
            entry.access == FileSystemAccessMode::Read
                && entry.path == FileSystemPath::from(read.clone())
        }));
        assert!(entries.iter().any(|entry| {
            entry.access == FileSystemAccessMode::Write
                && entry.path == FileSystemPath::from(write.clone())
        }));
        assert!(
            entries
                .iter()
                .all(|entry| matches!(entry.path, FileSystemPath::Path { .. }))
        );
    }
}
