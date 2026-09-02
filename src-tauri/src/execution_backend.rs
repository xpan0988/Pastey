//! Platform mechanics behind the generic managed ExecutionWorld controller.
//!
//! Backends receive only already-authorized mounts and process launch data.
//! They cannot compile effects, mint resource authority, or finalize lineage.

#![allow(dead_code)] // Step 8 is the first live product attachment.

use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::Path,
    process::ExitStatus,
    sync::Arc,
};

#[cfg(target_os = "macos")]
use std::fs;
#[cfg(unix)]
use std::process::{Command, Stdio};

use crate::{
    effect_authority::{ConfinementPropertyV1, ExecutionWorldRefV1},
    error::{AppError, AppResult},
    execution_world::{
        ExecutionWorldAvailabilityV1, ManagedProcessInvocationV1, PlatformWorldKindV1,
    },
    managed_resources::ExecutionWorldMountV1,
};

#[cfg(target_os = "macos")]
use crate::execution_world::{domain_hash, EXECUTION_WORLD_VERSION};

pub(crate) trait PlatformExecutionBackendV1: Send + Sync {
    fn availability(
        &self,
        required: &BTreeSet<ConfinementPropertyV1>,
    ) -> ExecutionWorldAvailabilityV1;

    fn prepare_world(
        &self,
        availability: &ExecutionWorldAvailabilityV1,
        world_ref: &ExecutionWorldRefV1,
        mounts: &[ExecutionWorldMountV1],
    ) -> AppResult<PreparedPlatformWorldV1>;
}

pub(crate) trait PlatformExecutionWorldV1: Send + Sync {
    fn spawn(&self, launch: PlatformProcessLaunchV1<'_>) -> AppResult<SpawnedPlatformProcessV1>;
}

pub(crate) trait PlatformProcessV1: Send {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn request_termination(&mut self);

    fn close_stdin(&mut self) {}

    fn resident_memory_bytes(&self) -> Option<u64> {
        None
    }
}

pub(crate) struct PreparedPlatformWorldV1 {
    pub(crate) mounts: Vec<ExecutionWorldMountV1>,
    pub(crate) world: Arc<dyn PlatformExecutionWorldV1>,
}

pub(crate) struct SpawnedPlatformProcessV1 {
    pub(crate) process: Box<dyn PlatformProcessV1>,
    pub(crate) stdin: Option<Box<dyn Write + Send>>,
    pub(crate) stdout: Box<dyn Read + Send>,
    pub(crate) stderr: Box<dyn Read + Send>,
}

pub(crate) struct PlatformProcessLaunchV1<'a> {
    pub(crate) mounts: &'a [ExecutionWorldMountV1],
    pub(crate) executable: &'a ExecutionWorldMountV1,
    pub(crate) invocation: &'a ManagedProcessInvocationV1,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) cpu_millis: u64,
    pub(crate) memory_bytes: u64,
    pub(crate) write_bytes: u64,
}

pub(crate) fn host_platform_execution_backend() -> Arc<dyn PlatformExecutionBackendV1> {
    #[cfg(windows)]
    return Arc::new(crate::windows_codex_backend::WindowsCodexBackendV1);
    #[cfg(not(windows))]
    Arc::new(HostPlatformExecutionBackendV1)
}

struct HostPlatformExecutionBackendV1;

impl PlatformExecutionBackendV1 for HostPlatformExecutionBackendV1 {
    fn availability(
        &self,
        required: &BTreeSet<ConfinementPropertyV1>,
    ) -> ExecutionWorldAvailabilityV1 {
        #[cfg(windows)]
        let _ = required;
        #[cfg(target_os = "macos")]
        return macos_availability(required);
        #[cfg(target_os = "linux")]
        return linux_availability();
        #[allow(unreachable_code)]
        ExecutionWorldAvailabilityV1 {
            kind: PlatformWorldKindV1::Unsupported,
            available: false,
            identity_digest: "pastey-platform-world-unavailable-v1".into(),
            verified_properties: BTreeSet::new(),
            unavailable_reason: Some("This Host platform has no verified execution world.".into()),
        }
    }

    fn prepare_world(
        &self,
        availability: &ExecutionWorldAvailabilityV1,
        world_ref: &ExecutionWorldRefV1,
        mounts: &[ExecutionWorldMountV1],
    ) -> AppResult<PreparedPlatformWorldV1> {
        if !availability.available {
            return unavailable("The selected platform execution backend is unavailable.");
        }
        #[cfg(unix)]
        if matches!(
            availability.kind,
            PlatformWorldKindV1::MacOsSandboxExec | PlatformWorldKindV1::LinuxBubblewrapCgroupV2
        ) {
            return Ok(PreparedPlatformWorldV1 {
                mounts: mounts.to_vec(),
                world: Arc::new(UnixPlatformWorldV1 {
                    kind: availability.kind,
                }),
            });
        }
        let _ = (world_ref, mounts);
        unavailable("No verified platform execution backend is available.")
    }
}

#[cfg(unix)]
struct UnixPlatformWorldV1 {
    kind: PlatformWorldKindV1,
}

#[cfg(unix)]
impl PlatformExecutionWorldV1 for UnixPlatformWorldV1 {
    fn spawn(&self, launch: PlatformProcessLaunchV1<'_>) -> AppResult<SpawnedPlatformProcessV1> {
        let mut command = build_unix_command(self.kind, &launch)?;
        command
            .env_clear()
            .stdin(if launch.invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &launch.invocation.environment {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|_| {
            AppError::InvalidInput("Verified execution world failed to spawn.".into())
        })?;
        let process_group = child.id() as i32;
        Ok(SpawnedPlatformProcessV1 {
            stdin: child
                .stdin
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Write + Send>),
            stdout: Box::new(child.stdout.take().ok_or_else(|| {
                AppError::InvalidInput("Contained process stdout pipe is unavailable.".into())
            })?),
            stderr: Box::new(child.stderr.take().ok_or_else(|| {
                AppError::InvalidInput("Contained process stderr pipe is unavailable.".into())
            })?),
            process: Box::new(UnixPlatformProcessV1 {
                child,
                process_group,
            }),
        })
    }
}

#[cfg(unix)]
struct UnixPlatformProcessV1 {
    child: std::process::Child,
    process_group: i32,
}

#[cfg(unix)]
impl PlatformProcessV1 for UnixPlatformProcessV1 {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn request_termination(&mut self) {
        unsafe {
            // A negative pid addresses exactly the process group created by
            // this backend. No requester-controlled Host pid is used.
            let _ = libc::kill(-self.process_group, libc::SIGKILL);
        };
    }

    #[cfg(target_os = "macos")]
    fn resident_memory_bytes(&self) -> Option<u64> {
        Some(macos_process_memory_bytes(self.process_group))
    }
}

pub(crate) fn exit_budget_state(status: &ExitStatus) -> Option<&'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        return match status.signal() {
            Some(libc::SIGXFSZ) => Some("resource_budget_exceeded"),
            Some(libc::SIGXCPU) => Some("cpu_budget_exceeded"),
            _ => None,
        };
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_availability(required: &BTreeSet<ConfinementPropertyV1>) -> ExecutionWorldAvailabilityV1 {
    let path = Path::new("/usr/bin/sandbox-exec");
    let result = (|| -> AppResult<String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return invalid("macOS sandbox-exec identity is unsafe.");
        }
        let bytes = fs::read(path)?;
        let probe = Command::new(path)
            .args([
                "-p",
                "(version 1)(deny default)(deny network*)(allow process-exec)(allow file-read* (literal \"/usr/bin/true\") (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\"))",
                "/usr/bin/true",
            ])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !probe.success() {
            return unavailable("The macOS sandbox behavioral probe failed.");
        }
        domain_hash(
            "pastey-macos-sandbox-exec-world-v1",
            &(
                blake3::hash(&bytes).to_hex().to_string(),
                EXECUTION_WORLD_VERSION,
                required,
            ),
        )
    })();
    match result {
        Ok(identity_digest) => ExecutionWorldAvailabilityV1 {
            kind: PlatformWorldKindV1::MacOsSandboxExec,
            available: true,
            identity_digest,
            verified_properties: required.clone(),
            unavailable_reason: None,
        },
        Err(_) => ExecutionWorldAvailabilityV1 {
            kind: PlatformWorldKindV1::MacOsSandboxExec,
            available: false,
            identity_digest: "pastey-macos-sandbox-exec-unavailable-v1".into(),
            verified_properties: BTreeSet::new(),
            unavailable_reason: Some("sandbox-exec is missing or has an unsafe identity.".into()),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_availability() -> ExecutionWorldAvailabilityV1 {
    ExecutionWorldAvailabilityV1 {
        kind: PlatformWorldKindV1::LinuxBubblewrapCgroupV2,
        available: false,
        identity_digest: "pastey-linux-bubblewrap-cgroup-v2-unavailable-v1".into(),
        verified_properties: BTreeSet::new(),
        unavailable_reason: Some(
            "The bubblewrap adapter remains unavailable until delegated cgroup-v2 attachment and native Linux conformance are implemented and verified."
                .into(),
        ),
    }
}

#[cfg(unix)]
fn build_unix_command(
    kind: PlatformWorldKindV1,
    launch: &PlatformProcessLaunchV1<'_>,
) -> AppResult<Command> {
    #[cfg(target_os = "macos")]
    if kind == PlatformWorldKindV1::MacOsSandboxExec {
        let profile = macos_profile(launch.mounts, launch.executable)?;
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-p")
            .arg(profile)
            .arg(&launch.executable.source_path)
            .args(&launch.invocation.argv);
        if let Some(cwd) = launch.cwd {
            command.current_dir(cwd);
        }
        configure_unix_process(&mut command, launch)?;
        return Ok(command);
    }
    #[cfg(target_os = "linux")]
    if kind == PlatformWorldKindV1::LinuxBubblewrapCgroupV2 {
        let bwrap = if Path::new("/usr/bin/bwrap").is_file() {
            "/usr/bin/bwrap"
        } else {
            "/bin/bwrap"
        };
        let mut command = Command::new(bwrap);
        command.args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--proc",
            "/proc",
            "--dir",
            "/dev",
            "--dir",
            "/tmp",
        ]);
        for mount in launch.mounts {
            let target = format!("/pastey/resources/{}", mount.mount_name);
            command
                .arg(if mount.writable {
                    "--bind"
                } else {
                    "--ro-bind"
                })
                .arg(&mount.source_path)
                .arg(target);
        }
        let target_executable = format!("/pastey/resources/{}", launch.executable.mount_name);
        if let (Some(handle), Some(selector)) = (
            launch.invocation.working_directory_handle.as_ref(),
            launch.invocation.working_directory_selector.as_deref(),
        ) {
            let mount = launch
                .mounts
                .iter()
                .find(|mount| mount.handle_ref == *handle)
                .ok_or_else(|| {
                    AppError::InvalidInput("Working directory mount is unavailable.".into())
                })?;
            let mut target = format!("/pastey/resources/{}", mount.mount_name);
            if selector != "." {
                target.push('/');
                target.push_str(selector);
            }
            command.arg("--chdir").arg(target);
        }
        command
            .arg("--")
            .arg(target_executable)
            .args(&launch.invocation.argv);
        configure_unix_process(&mut command, launch)?;
        return Ok(command);
    }
    let _ = (kind, launch);
    unavailable("No verified platform execution backend is available.")
}

#[cfg(target_os = "macos")]
fn macos_profile(
    mounts: &[ExecutionWorldMountV1],
    executable: &ExecutionWorldMountV1,
) -> AppResult<String> {
    fn literal(path: &Path) -> AppResult<String> {
        let value = path.to_str().ok_or_else(|| {
            AppError::InvalidInput("Execution world path is not valid UTF-8.".into())
        })?;
        Ok(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }
    let mut profile = String::from(
        "(version 1)\n(deny default)\n(deny network*)\n(allow signal (target self))\n(allow sysctl-read)\n(allow file-read* (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\"))\n",
    );
    profile.push_str(&format!(
        "(allow file-read* (literal {0}))\n(allow process-exec (literal {0}))\n",
        literal(&executable.source_path)?
    ));
    if let Ok(canonical) = executable.source_path.canonicalize() {
        profile.push_str(&format!(
            "(allow file-read* (literal {0}))\n(allow process-exec (literal {0}))\n",
            literal(&canonical)?
        ));
    }
    for mount in mounts {
        let path = literal(&mount.source_path)?;
        profile.push_str(&format!(
            "(allow file-read* (subpath {path}) (literal {path}))\n"
        ));
        if mount.writable {
            profile.push_str(&format!(
                "(allow file-write* (subpath {path}) (literal {path}))\n"
            ));
        }
    }
    Ok(profile)
}

#[cfg(unix)]
fn configure_unix_process(
    command: &mut Command,
    launch: &PlatformProcessLaunchV1<'_>,
) -> AppResult<()> {
    use std::os::unix::process::CommandExt;
    let cpu_seconds = launch.cpu_millis.saturating_add(999) / 1000;
    if cpu_seconds == 0 || launch.memory_bytes < 1024 * 1024 {
        return unavailable("Reserved process CPU or memory budget cannot form a safe limit.");
    }
    let memory_bytes = launch.memory_bytes;
    let write_bytes = launch.write_bytes;
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            set_limit(libc::RLIMIT_CPU, cpu_seconds, cpu_seconds)?;
            #[cfg(target_os = "macos")]
            let _ = memory_bytes;
            #[cfg(not(target_os = "macos"))]
            set_limit(libc::RLIMIT_AS, memory_bytes, memory_bytes)?;
            set_limit(libc::RLIMIT_NOFILE, 32, 32)?;
            set_limit(libc::RLIMIT_FSIZE, write_bytes, write_bytes)?;
            let maximum_fd = libc::sysconf(libc::_SC_OPEN_MAX).clamp(3, 65_536);
            for fd in 3..maximum_fd {
                libc::close(fd as i32);
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(unix)]
unsafe fn set_limit(resource: libc::c_int, soft: u64, hard: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: soft as libc::rlim_t,
        rlim_max: hard as libc::rlim_t,
    };
    if libc::setrlimit(resource as _, &limit) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_process_memory_bytes(process_group: i32) -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        csw: i32,
        threadnum: i32,
        numrunning: i32,
        priority: i32,
    }
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let mut info = ProcTaskInfo::default();
    let size = std::mem::size_of::<ProcTaskInfo>() as i32;
    let read = unsafe {
        proc_pidinfo(
            process_group,
            4,
            0,
            (&mut info as *mut ProcTaskInfo).cast(),
            size,
        )
    };
    if read == size {
        info.resident_size
    } else {
        // Losing resource observability is itself fail-closed.
        u64::MAX
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn unavailable<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_backend_reports_complete_or_unavailable_without_fallback() {
        let required = crate::execution_world::required_properties();
        let backend = host_platform_execution_backend();
        let availability = backend.availability(&required);
        if availability.available {
            assert!(availability.verified_properties.is_superset(&required));
            assert!(availability.unavailable_reason.is_none());
        } else {
            assert!(availability.verified_properties.is_empty());
            assert!(availability.unavailable_reason.is_some());
            let world_ref = serde_json::from_value(serde_json::json!("unavailable-world"))
                .expect("test world ref");
            assert!(backend
                .prepare_world(&availability, &world_ref, &[])
                .is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn selected_windows_backend_is_the_codex_adapter() {
        let required = crate::execution_world::required_properties();
        assert_eq!(
            host_platform_execution_backend().availability(&required),
            crate::windows_codex_backend::WindowsCodexBackendV1.availability(&required)
        );
    }
}
