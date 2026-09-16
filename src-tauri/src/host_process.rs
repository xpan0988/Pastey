//! Host-owned bounded process-tree mechanics.
//!
//! Callers provide an already-qualified executable plus their own arguments,
//! environment, cwd, and protocol interpretation. This module owns only the
//! mechanical containment and bounded collection needed by any Host process.
//! A Unix process group is a termination aid, not proof that a hostile child
//! cannot escape the tree; production callers must require a stronger proof.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::error::{AppError, AppResult};

const QUIESCENCE_POLLS: usize = 100;
const QUIESCENCE_POLL_MILLIS: u64 = 10;
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const ROOT_REAP_TIMEOUT: Duration = Duration::from_secs(1);

/// A production launch may not treat process-group disappearance as proof of
/// process-tree containment. The test-only relaxation exists solely to
/// exercise bounded process mechanics without manufacturing qualification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostProcessTreeRequirementV1 {
    RequireVerifiedTree,
    #[cfg(test)]
    AllowProcessGroupForTest,
}

impl HostProcessTreeRequirementV1 {
    fn permits_process_group_only(self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::AllowProcessGroupForTest) {
            return true;
        }
        false
    }
}

/// A future specialist controller can opt into this Host-private macOS
/// Seatbelt seam. No current specialist launch uses it, and it always denies
/// network: provider-specific network policy belongs to a later slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HostProcessSandboxV1 {
    None,
    #[cfg(target_os = "macos")]
    #[allow(dead_code)] // Deliberately staged for a later controller launch.
    MacOsSeatbelt(HostMacosSeatbeltProfileV1),
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostMacosSeatbeltProfileV1 {
    executable: PathBuf,
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
}

#[cfg(target_os = "macos")]
impl HostMacosSeatbeltProfileV1 {
    /// Creates the future controller profile shape from already-qualified
    /// paths. It deliberately provides no network exception.
    #[allow(dead_code)] // The seam is intentionally not wired in S1.
    pub(crate) fn network_denied(
        executable: PathBuf,
        readable_roots: Vec<PathBuf>,
        writable_roots: Vec<PathBuf>,
    ) -> AppResult<Self> {
        if !executable.is_absolute()
            || readable_roots.iter().any(|path| !path.is_absolute())
            || writable_roots.iter().any(|path| !path.is_absolute())
        {
            return invalid("macOS Host sandbox paths must be absolute.");
        }
        Ok(Self {
            executable,
            readable_roots,
            writable_roots,
        })
    }

    fn render(&self) -> AppResult<String> {
        let mut profile = String::from(
            "(version 1)\n(deny default)\n(deny network*)\n(allow signal (target self))\n(allow sysctl-read)\n(allow process-fork)\n(allow file-read* (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\"))\n",
        );
        append_read_and_exec(&mut profile, &self.executable)?;
        for root in &self.readable_roots {
            append_read_root(&mut profile, root)?;
        }
        for root in &self.writable_roots {
            append_read_root(&mut profile, root)?;
            let literal = seatbelt_literal(root)?;
            profile.push_str(&format!(
                "(allow file-write* (subpath {literal}) (literal {literal}))\n"
            ));
        }
        Ok(profile)
    }
}

#[cfg(target_os = "macos")]
fn seatbelt_literal(path: &Path) -> AppResult<String> {
    let value = path
        .to_str()
        .ok_or_else(|| AppError::InvalidInput("macOS sandbox path is not valid UTF-8.".into()))?;
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(target_os = "macos")]
fn append_read_root(profile: &mut String, path: &Path) -> AppResult<()> {
    let literal = seatbelt_literal(path)?;
    profile.push_str(&format!(
        "(allow file-read* (subpath {literal}) (literal {literal}))\n"
    ));
    Ok(())
}

#[cfg(target_os = "macos")]
fn append_read_and_exec(profile: &mut String, executable: &Path) -> AppResult<()> {
    let literal = seatbelt_literal(executable)?;
    profile.push_str(&format!(
        "(allow file-read* (literal {literal}))\n(allow process-exec (literal {literal}))\n"
    ));
    if let Ok(canonical) = executable.canonicalize() {
        let canonical = seatbelt_literal(&canonical)?;
        profile.push_str(&format!(
            "(allow file-read* (literal {canonical}))\n(allow process-exec (literal {canonical}))\n"
        ));
    }
    Ok(())
}

/// Verifies the stable Host-owned `sandbox-exec` launch dependency. The
/// caller owns any higher-level availability or semantic-profile proof.
#[cfg(target_os = "macos")]
pub(crate) fn macos_sandbox_exec_identity() -> AppResult<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let path = Path::new("/usr/bin/sandbox-exec");
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return invalid("macOS sandbox-exec identity is unsafe.");
    }
    Ok(blake3::hash(&fs::read(path)?).to_hex().to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_sandbox_exec_command(
    profile: String,
    executable: &Path,
    argv: &[String],
) -> AppResult<Command> {
    let _identity = macos_sandbox_exec_identity()?;
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.arg("-p").arg(profile).arg(executable).args(argv);
    Ok(command)
}

#[cfg(target_os = "macos")]
fn macos_sandbox_command(
    profile: &HostMacosSeatbeltProfileV1,
    argv: &[String],
) -> AppResult<Command> {
    macos_sandbox_exec_command(profile.render()?, &profile.executable, argv)
}

/// One explicit Host process launch. The caller owns executable identity,
/// arguments, environment contents, working directory, and success meaning.
pub(crate) struct HostBoundedProcessSpecV1 {
    pub(crate) executable: PathBuf,
    pub(crate) argv: Vec<String>,
    pub(crate) current_dir: Option<PathBuf>,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) stdin: Stdio,
    pub(crate) stdout: Stdio,
    pub(crate) stderr: Stdio,
    pub(crate) stdout_limit: usize,
    pub(crate) stderr_limit: usize,
    pub(crate) wall_timeout: Duration,
    pub(crate) tree_requirement: HostProcessTreeRequirementV1,
    pub(crate) sandbox: HostProcessSandboxV1,
    /// Host-private directories created solely for this launch. They are
    /// removed only after the required containment proof is available.
    pub(crate) cleanup_roots: Vec<PathBuf>,
}

#[derive(Debug)]
struct HostProcessControlStateV1 {
    #[cfg(unix)]
    process_group: i32,
    armed: AtomicBool,
}

#[derive(Clone, Debug)]
pub(crate) struct HostProcessControlV1 {
    state: Arc<HostProcessControlStateV1>,
}

impl HostProcessControlV1 {
    pub(crate) fn terminate(&self) {
        #[cfg(unix)]
        if self.state.armed.load(Ordering::SeqCst) {
            terminate_process_group(self.state.process_group);
        }
    }

    /// This verifies only the original process group. It is intentionally not
    /// named process-tree quiescence: a descendant may have escaped with
    /// `setsid` or `setpgid`.
    pub(crate) fn wait_for_quiescence(&self) -> bool {
        #[cfg(unix)]
        return wait_for_process_group_quiescence(self.state.process_group);
        #[cfg(not(unix))]
        false
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        #[cfg(unix)]
        return !process_group_is_alive(self.state.process_group);
        #[cfg(not(unix))]
        false
    }

    pub(crate) fn process_tree_quiescence_is_proven(&self) -> bool {
        // S1 deliberately does not manufacture a proof from a Unix process
        // group. A later platform-specific descendant tracker may change this.
        false
    }

    fn disarm_if_group_quiescent(&self) -> bool {
        if self.wait_for_quiescence() {
            self.state.armed.store(false, Ordering::SeqCst);
            return true;
        }
        false
    }

    fn terminate_and_wait_for_group_quiescence(&self) -> bool {
        self.terminate();
        self.disarm_if_group_quiescent()
    }
}

/// A started process whose original process group remains under Host control.
/// It cannot return accepted output where the requested tree proof is absent.
pub(crate) struct RunningHostProcessV1 {
    child: Child,
    control: HostProcessControlV1,
    cleanup_roots: Vec<PathBuf>,
    stdout_limit: usize,
    stderr_limit: usize,
    wall_deadline: Instant,
    tree_requirement: HostProcessTreeRequirementV1,
    finalized: bool,
}

impl RunningHostProcessV1 {
    pub(crate) fn control(&self) -> HostProcessControlV1 {
        self.control.clone()
    }

    /// Waits for the root process, bounds collection and wall clock, then
    /// requires the configured process-tree proof before returning output.
    pub(crate) fn wait(mut self) -> AppResult<Output> {
        let result = self.wait_bounded();
        if result.is_err() {
            // Stream, wall-clock, protocol-adjacent, or proof failures are
            // cancellation points. Never rely on Drop after consuming `self`.
            self.control.terminate_and_wait_for_group_quiescence();
            #[cfg(unix)]
            let _ = reap_root(&mut self.child);
        }
        self.cleanup_after_terminal_result();
        self.finalized = true;
        result
    }

    fn wait_bounded(&mut self) -> AppResult<Output> {
        #[cfg(unix)]
        {
            return self.wait_bounded_unix();
        }
        #[cfg(not(unix))]
        invalid("Host bounded process groups are unavailable on this platform.")
    }

    #[cfg(unix)]
    fn wait_bounded_unix(&mut self) -> AppResult<Output> {
        use std::os::fd::AsRawFd;

        let mut stdout = self.child.stdout.take().ok_or_else(|| {
            AppError::InvalidInput("Host process stdout pipe is unavailable.".into())
        })?;
        let mut stderr = self.child.stderr.take().ok_or_else(|| {
            AppError::InvalidInput("Host process stderr pipe is unavailable.".into())
        })?;
        set_nonblocking(stdout.as_raw_fd())?;
        set_nonblocking(stderr.as_raw_fd())?;

        let mut collected_stdout = Vec::new();
        let mut collected_stderr = Vec::new();
        let mut stdout_eof = false;
        let mut stderr_eof = false;
        loop {
            drain_nonblocking(
                &mut stdout,
                &mut collected_stdout,
                self.stdout_limit,
                &mut stdout_eof,
            )?;
            drain_nonblocking(
                &mut stderr,
                &mut collected_stderr,
                self.stderr_limit,
                &mut stderr_eof,
            )?;

            match self.child.try_wait() {
                Ok(Some(status)) => {
                    if !self.control.disarm_if_group_quiescent() {
                        self.control.terminate_and_wait_for_group_quiescence();
                        let _ = reap_root(&mut self.child);
                        return invalid("Host process descendants survived root-process exit.");
                    }
                    let drain_deadline = Instant::now() + PIPE_DRAIN_TIMEOUT;
                    while !stdout_eof || !stderr_eof {
                        drain_nonblocking(
                            &mut stdout,
                            &mut collected_stdout,
                            self.stdout_limit,
                            &mut stdout_eof,
                        )?;
                        drain_nonblocking(
                            &mut stderr,
                            &mut collected_stderr,
                            self.stderr_limit,
                            &mut stderr_eof,
                        )?;
                        if stdout_eof && stderr_eof {
                            break;
                        }
                        if Instant::now() >= drain_deadline {
                            return invalid(
                                "Host process output pipe remained live after group quiescence.",
                            );
                        }
                        thread::sleep(Duration::from_millis(QUIESCENCE_POLL_MILLIS));
                    }
                    if !self.tree_requirement.permits_process_group_only()
                        && !self.control.process_tree_quiescence_is_proven()
                    {
                        return invalid(
                            "Host process group quiescence does not prove hostile process-tree containment.",
                        );
                    }
                    return Ok(Output {
                        status,
                        stdout: collected_stdout,
                        stderr: collected_stderr,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    self.control.terminate_and_wait_for_group_quiescence();
                    let _ = reap_root(&mut self.child);
                    return Err(error.into());
                }
            }
            if Instant::now() >= self.wall_deadline {
                let group_quiescent = self.control.terminate_and_wait_for_group_quiescence();
                let root_reaped = reap_root(&mut self.child);
                if !group_quiescent || !root_reaped {
                    return invalid("Host process timeout left containment unproven.");
                }
                return invalid("Host process exceeded its wall-clock timeout.");
            }
            thread::sleep(Duration::from_millis(QUIESCENCE_POLL_MILLIS));
        }
    }

    fn cleanup_after_terminal_result(&mut self) {
        let containment_proven = self.control.process_tree_quiescence_is_proven()
            || self.tree_requirement.permits_process_group_only();
        if containment_proven && self.control.is_quiescent() {
            self.cleanup_roots.drain(..).for_each(|root| {
                let _ = fs::remove_dir_all(root);
            });
        }
    }
}

impl Drop for RunningHostProcessV1 {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        // Dropping an unobserved controller is cancellation. Cleanup remains
        // fail-closed: private roots are retained if tree proof is unavailable.
        self.control.terminate_and_wait_for_group_quiescence();
        #[cfg(unix)]
        let _ = reap_root(&mut self.child);
        self.cleanup_after_terminal_result();
    }
}

/// Launches one caller-specified process in a new Host-owned process group.
/// macOS/Unix containment is required until an equivalent Windows Job Object
/// implementation is available.
pub(crate) fn spawn_bounded_host_process(
    spec: HostBoundedProcessSpecV1,
) -> AppResult<RunningHostProcessV1> {
    if spec.wall_timeout.is_zero() {
        return invalid("Host process wall-clock timeout must be nonzero.");
    }
    #[cfg(not(unix))]
    {
        let _ = spec;
        return invalid("Host bounded process groups are unavailable on this platform.");
    }
    #[cfg(unix)]
    {
        let mut command = host_command_for_spec(&spec)?;
        command
            .env_clear()
            .envs(&spec.environment)
            .stdin(spec.stdin)
            .stdout(spec.stdout)
            .stderr(spec.stderr);
        if let Some(current_dir) = &spec.current_dir {
            command.current_dir(current_dir);
        }
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok(RunningHostProcessV1 {
            control: HostProcessControlV1 {
                state: Arc::new(HostProcessControlStateV1 {
                    process_group: child.id() as i32,
                    armed: AtomicBool::new(true),
                }),
            },
            child,
            cleanup_roots: spec.cleanup_roots,
            stdout_limit: spec.stdout_limit,
            stderr_limit: spec.stderr_limit,
            wall_deadline: Instant::now() + spec.wall_timeout,
            tree_requirement: spec.tree_requirement,
            finalized: false,
        })
    }
}

#[cfg(unix)]
fn host_command_for_spec(spec: &HostBoundedProcessSpecV1) -> AppResult<Command> {
    #[cfg(target_os = "macos")]
    if let HostProcessSandboxV1::MacOsSeatbelt(profile) = &spec.sandbox {
        if profile.executable != spec.executable {
            return invalid("macOS Host sandbox executable does not match the launch spec.");
        }
        return macos_sandbox_command(profile, &spec.argv);
    }
    #[cfg(not(target_os = "macos"))]
    if !matches!(spec.sandbox, HostProcessSandboxV1::None) {
        return invalid("The requested Host process sandbox is unavailable on this platform.");
    }
    let mut command = Command::new(&spec.executable);
    command.args(&spec.argv);
    Ok(command)
}

#[cfg(unix)]
fn set_nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain_nonblocking<R: io::Read>(
    stream: &mut R,
    collected: &mut Vec<u8>,
    limit: usize,
    eof: &mut bool,
) -> AppResult<()> {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                *eof = true;
                return Ok(());
            }
            Ok(count) => {
                if collected.len().saturating_add(count) > limit {
                    return invalid("Host process output exceeded its configured limit.");
                }
                collected.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(unix)]
fn reap_root(child: &mut Child) -> bool {
    let deadline = Instant::now() + ROOT_REAP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(QUIESCENCE_POLL_MILLIS));
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: i32) {
    unsafe {
        let _ = libc::kill(-process_group, libc::SIGTERM);
        let _ = libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn process_group_is_alive(process_group: i32) -> bool {
    if unsafe { libc::kill(-process_group, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
fn wait_for_process_group_quiescence(process_group: i32) -> bool {
    for _ in 0..QUIESCENCE_POLLS {
        if !process_group_is_alive(process_group) {
            return true;
        }
        thread::sleep(Duration::from_millis(QUIESCENCE_POLL_MILLIS));
    }
    !process_group_is_alive(process_group)
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    fn shell(
        script: &str,
        stdout_limit: usize,
        stderr_limit: usize,
        tree_requirement: HostProcessTreeRequirementV1,
    ) -> RunningHostProcessV1 {
        spawn_bounded_host_process(HostBoundedProcessSpecV1 {
            executable: PathBuf::from("/bin/sh"),
            argv: vec!["-c".into(), script.into()],
            current_dir: None,
            environment: BTreeMap::new(),
            stdin: Stdio::null(),
            stdout: Stdio::piped(),
            stderr: Stdio::piped(),
            stdout_limit,
            stderr_limit,
            wall_timeout: TEST_TIMEOUT,
            tree_requirement,
            sandbox: HostProcessSandboxV1::None,
            cleanup_roots: vec![],
        })
        .unwrap()
    }

    fn test_group_requirement() -> HostProcessTreeRequirementV1 {
        HostProcessTreeRequirementV1::AllowProcessGroupForTest
    }

    #[test]
    fn bounded_stream_overflow_terminates_the_complete_group() {
        let process = shell("yes x", 64, 64, test_group_requirement());
        let control = process.control();
        assert!(process.wait().is_err());
        assert!(control.wait_for_quiescence());
    }

    #[test]
    fn root_exit_with_a_live_background_child_fails_closed() {
        assert!(shell(
            "sleep 30 & printf root",
            1024,
            1024,
            test_group_requirement()
        )
        .wait()
        .is_err());
    }

    #[test]
    fn wall_timeout_terminates_the_group_and_returns_no_output() {
        assert!(shell("sleep 30", 1024, 1024, test_group_requirement())
            .wait()
            .is_err());
    }

    #[test]
    fn production_requirement_rejects_group_only_quiescence() {
        assert!(shell(
            "printf complete",
            1024,
            1024,
            HostProcessTreeRequirementV1::RequireVerifiedTree,
        )
        .wait()
        .is_err());
    }

    #[test]
    fn background_and_grandchild_probes_fail_closed() {
        for script in [
            "(sleep 30) & printf root",
            "(sh -c 'sleep 30 & wait') & printf root",
        ] {
            assert!(shell(script, 1024, 1024, test_group_requirement())
                .wait()
                .is_err());
        }
    }

    #[test]
    fn setsid_and_setpgid_escape_probes_fail_closed() {
        // `/usr/bin/perl` is supplied by macOS and common Unix developer
        // hosts; each descendant retains stdout so the Host detects that its
        // original group is not sufficient proof. The process is explicitly
        // reaped by the test because that is precisely what S1 cannot claim.
        for perl in [
            "setsid() or die $!; sleep 30",
            "setpgid(0, 0) == 0 or die $!; sleep 30",
        ] {
            let marker =
                std::env::temp_dir().join(format!("pastey-host-escape-{}", uuid::Uuid::new_v4()));
            let script = format!(
                "(/usr/bin/perl -MPOSIX=setsid,setpgid -e '{}' ) & pid=$!; printf '%s' \"$pid\" > '{}'; wait",
                perl,
                marker.to_string_lossy(),
            );
            let process = shell(&script, 1024, 1024, test_group_requirement());
            let result = process.wait();
            assert!(result.is_err());
            let pid = fs::read_to_string(&marker).unwrap().parse::<i32>().unwrap();
            unsafe {
                let _ = libc::kill(pid, libc::SIGKILL);
            }
            let _ = fs::remove_file(marker);
        }
    }

    #[test]
    fn delayed_descendant_probe_fails_closed() {
        assert!(shell(
            "(sleep 1; sleep 30) & printf root",
            1024,
            1024,
            test_group_requirement(),
        )
        .wait()
        .is_err());
    }

    #[test]
    fn dropping_a_running_process_cancels_its_group() {
        let process = shell("sleep 30 & wait", 1024, 1024, test_group_requirement());
        let control = process.control();
        drop(process);
        assert!(control.wait_for_quiescence());
    }
}
