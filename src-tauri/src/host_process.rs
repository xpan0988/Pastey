//! Host-owned bounded process-tree mechanics.
//!
//! Callers provide an already-qualified executable plus their own arguments,
//! environment, cwd, and protocol interpretation. This module owns only the
//! mechanical containment and bounded collection needed by any Host process.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::error::{AppError, AppResult};

const QUIESCENCE_POLLS: usize = 100;
const QUIESCENCE_POLL_MILLIS: u64 = 10;

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
    /// Host-private directories created solely for this launch. They are
    /// removed after the process tree has been reaped or terminated.
    pub(crate) cleanup_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct HostProcessControlV1 {
    #[cfg(unix)]
    process_group: i32,
}

impl HostProcessControlV1 {
    pub(crate) fn terminate(&self) {
        #[cfg(unix)]
        terminate_process_group(self.process_group);
    }

    pub(crate) fn wait_for_quiescence(&self) -> bool {
        #[cfg(unix)]
        return wait_for_process_group_quiescence(self.process_group);
        #[cfg(not(unix))]
        false
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        #[cfg(unix)]
        return !process_group_is_alive(self.process_group);
        #[cfg(not(unix))]
        false
    }

    fn terminate_and_wait_for_quiescence(&self) {
        self.terminate();
        let _ = self.wait_for_quiescence();
    }
}

/// A started process whose complete process group remains under Host control.
pub(crate) struct RunningHostProcessV1 {
    child: Child,
    control: HostProcessControlV1,
    cleanup_roots: Vec<PathBuf>,
    stdout_limit: usize,
    stderr_limit: usize,
}

impl RunningHostProcessV1 {
    pub(crate) fn control(&self) -> HostProcessControlV1 {
        self.control.clone()
    }

    /// Waits for the root process, proves the group is empty before draining
    /// retained pipes, and bounds both streams while they are being read.
    pub(crate) fn wait(mut self) -> AppResult<Output> {
        let result = self.wait_bounded();
        for root in self.cleanup_roots {
            let _ = fs::remove_dir_all(root);
        }
        result
    }

    fn wait_bounded(&mut self) -> AppResult<Output> {
        let stdout = self.child.stdout.take().ok_or_else(|| {
            AppError::InvalidInput("Host process stdout pipe is unavailable.".into())
        })?;
        let stderr = self.child.stderr.take().ok_or_else(|| {
            AppError::InvalidInput("Host process stderr pipe is unavailable.".into())
        })?;
        let stdout_overflow = Arc::new(AtomicBool::new(false));
        let stderr_overflow = Arc::new(AtomicBool::new(false));
        let stdout_reader = spawn_bounded_reader(
            stdout,
            self.stdout_limit,
            stdout_overflow.clone(),
            self.control.clone(),
        );
        let stderr_reader = spawn_bounded_reader(
            stderr,
            self.stderr_limit,
            stderr_overflow.clone(),
            self.control.clone(),
        );
        let status = match self.child.wait() {
            Ok(status) => status,
            Err(error) => {
                self.control.terminate_and_wait_for_quiescence();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error.into());
            }
        };
        // A descendant can retain either pipe after the root has exited. Do
        // not wait indefinitely for EOF or treat root exit as tree quiescence.
        if !self.control.is_quiescent() {
            self.control.terminate_and_wait_for_quiescence();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return invalid("Host process descendants survived root-process exit.");
        }
        let stdout = join_bounded_reader(stdout_reader);
        let stderr = join_bounded_reader(stderr_reader);
        if stdout_overflow.load(Ordering::SeqCst) || stderr_overflow.load(Ordering::SeqCst) {
            self.control.terminate_and_wait_for_quiescence();
            return invalid("Host process output exceeded its configured limit.");
        }
        let stdout = match stdout {
            Ok(output) => output,
            Err(error) => {
                self.control.terminate_and_wait_for_quiescence();
                return Err(error);
            }
        };
        let stderr = match stderr {
            Ok(output) => output,
            Err(error) => {
                self.control.terminate_and_wait_for_quiescence();
                return Err(error);
            }
        };
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// Launches one caller-specified process in a new Host-owned process group.
/// macOS/Unix containment is required until an equivalent Windows Job Object
/// implementation is available.
pub(crate) fn spawn_bounded_host_process(
    spec: HostBoundedProcessSpecV1,
) -> AppResult<RunningHostProcessV1> {
    #[cfg(not(unix))]
    {
        let _ = spec;
        return invalid("Host bounded process groups are unavailable on this platform.");
    }
    #[cfg(unix)]
    {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.argv)
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
                process_group: child.id() as i32,
            },
            child,
            cleanup_roots: spec.cleanup_roots,
            stdout_limit: spec.stdout_limit,
            stderr_limit: spec.stderr_limit,
        })
    }
}

fn spawn_bounded_reader<R: Read + Send + 'static>(
    stream: R,
    limit: usize,
    overflow: Arc<AtomicBool>,
    control: HostProcessControlV1,
) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut stream = stream;
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => {
                    control.terminate();
                    return Err(error);
                }
            };
            if count == 0 {
                return Ok(collected);
            }
            if collected.len().saturating_add(count) > limit {
                overflow.store(true, Ordering::SeqCst);
                control.terminate();
                return Err(io::Error::other(
                    "Host process output exceeded its configured limit.",
                ));
            }
            collected.extend_from_slice(&buffer[..count]);
        }
    })
}

fn join_bounded_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> AppResult<Vec<u8>> {
    match reader.join() {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) => invalid("Host process output stream failed."),
        Err(_) => invalid("Host process output reader panicked."),
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: i32) {
    unsafe {
        libc::kill(-process_group, libc::SIGTERM);
        libc::kill(-process_group, libc::SIGKILL);
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

    fn shell(script: &str, stdout_limit: usize, stderr_limit: usize) -> RunningHostProcessV1 {
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
            cleanup_roots: vec![],
        })
        .unwrap()
    }

    #[test]
    fn bounded_stream_overflow_terminates_the_complete_tree() {
        assert!(shell("yes x", 64, 64).wait().is_err());
    }

    #[test]
    fn root_exit_with_a_live_descendant_fails_closed() {
        assert!(shell("sleep 30 & printf root", 1024, 1024).wait().is_err());
    }
}
