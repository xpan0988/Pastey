//! The lifeline of a Claude Code `-p` run (Unix).
//!
//! `claude -p` does not stop when the process that started it dies: it
//! finishes its turn unobserved (measured with real tool calls on 2.1.317).
//! So Pastey never starts it directly. It starts its own executable again in
//! this helper mode, and the helper starts `claude` as its child, in a
//! process group of its own, and stays its parent until it exits.
//!
//! Pastey keeps the write end of a pipe, the helper's descriptor 3, and
//! writes one byte to ask for a signal. When the pipe closes, as it does
//! however Pastey's process ends, the helper interrupts the run as Stop
//! would (SIGINT), then escalates: SIGTERM, then SIGKILL to the process
//! group. Measured on macOS with a real Bash tool: the tool was gone within
//! 1 s and `claude` within 5 s.
//!
//! `claude`'s stdin, stdout and stderr are the helper's own, so the stream
//! passes straight between Pastey and `claude`. Only the parent signals the
//! child, and only while it has not reaped it, so a signal never reaches a
//! reused process id.

use std::{
    ffi::OsStr,
    io::{self, Read},
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, Command, ExitStatus},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// `argv[1]` that starts Pastey's executable as a lifeline helper. The
/// program and arguments to run follow it.
pub(crate) const LIFELINE_ARG: &str = "--pastey-claude-code-lifeline";
/// The helper's descriptor for the read end of the lifeline.
const LIFELINE_FD: i32 = 3;

/// Requests Pastey writes on the lifeline.
pub(in crate::native_agent) const INTERRUPT: u8 = b'i';
pub(in crate::native_agent) const TERMINATE: u8 = b't';
pub(in crate::native_agent) const KILL: u8 = b'k';

/// After the lifeline closes: how long an interrupted run may take to end,
/// then how long a terminated one may.
#[cfg(not(test))]
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
#[cfg(not(test))]
const TERMINATE_GRACE: Duration = Duration::from_secs(5);
#[cfg(test)]
const INTERRUPT_GRACE: Duration = Duration::from_millis(400);
#[cfg(test)]
const TERMINATE_GRACE: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(20);

/// In the helper process: runs what follows `LIFELINE_ARG` on the lifeline
/// and exits with its status. `false` when this process is not a helper.
pub(crate) fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(LIFELINE_ARG)) {
        return false;
    }
    let Some(program) = args.next() else {
        std::process::exit(2);
    };
    // SAFETY: Pastey started this helper with the lifeline's read end as
    // descriptor 3, and nothing else in this process uses it.
    let lifeline = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(LIFELINE_FD) };
    let code = match supervise(Command::new(program).args(args), lifeline) {
        Ok(status) => status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        Err(_) => 127,
    };
    std::process::exit(code);
}

/// Runs `command` in a process group of its own until it exits, signalling
/// it as `lifeline` asks, and stopping it when `lifeline` closes.
pub(in crate::native_agent) fn supervise(
    command: &mut Command,
    lifeline: impl Read + Send + 'static,
) -> io::Result<ExitStatus> {
    // SAFETY: only an async-signal-safe call between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = Arc::new(Mutex::new(command.spawn()?));
    {
        let child = child.clone();
        thread::spawn(move || watch(lifeline, &child));
    }
    loop {
        if let Some(status) = lock(&child).try_wait()? {
            return Ok(status);
        }
        thread::sleep(POLL);
    }
}

fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn watch(mut lifeline: impl Read, child: &Mutex<Child>) {
    let mut request = [0u8; 1];
    loop {
        match lifeline.read(&mut request) {
            Ok(1) => match request[0] {
                INTERRUPT => {
                    signal(child, libc::SIGINT, false);
                }
                TERMINATE => {
                    signal(child, libc::SIGTERM, false);
                }
                KILL => {
                    signal(child, libc::SIGKILL, true);
                }
                _ => {}
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            _ => break,
        }
    }
    // Pastey is gone: end the turn as Stop would, then make sure.
    for (signal_number, grace) in [
        (libc::SIGINT, INTERRUPT_GRACE),
        (libc::SIGTERM, TERMINATE_GRACE),
    ] {
        if !signal(child, signal_number, false) {
            return;
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if !matches!(lock(child).try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL);
        }
    }
    signal(child, libc::SIGKILL, true);
}

/// Signals the child (or its whole process group) only while it has not
/// been reaped. `false` once it has exited.
fn signal(child: &Mutex<Child>, signal_number: i32, group: bool) -> bool {
    let mut child = lock(child);
    if !matches!(child.try_wait(), Ok(None)) {
        return false;
    }
    let pid = child.id() as i32;
    // SAFETY: plain kill(2) on a child this process has not reaped.
    unsafe {
        libc::kill(if group { -pid } else { pid }, signal_number);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// A child that records each signal it gets, and keeps running after
    /// SIGINT unless told to stop on it.
    fn child_script(root: &Path, stops_on_interrupt: bool) -> PathBuf {
        let script = root.join("child.sh");
        let on_int = if stops_on_interrupt {
            "echo int >> \"$ROOT/signals\"; exit 0"
        } else {
            "echo int >> \"$ROOT/signals\""
        };
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nROOT='{}'\ntrap '{on_int}' INT\ntrap 'echo term >> \"$ROOT/signals\"' TERM\n\
                 echo started > \"$ROOT/started\"\nwhile :; do sleep 0.05; done\n",
                root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script
    }

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("pastey-lifeline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn supervised(script: PathBuf) -> (io::PipeWriter, thread::JoinHandle<ExitStatus>) {
        let (reader, writer) = io::pipe().unwrap();
        // Through /bin/sh: a script just written cannot then be "busy" to
        // exec while another test thread forks.
        let handle =
            thread::spawn(move || supervise(Command::new("/bin/sh").arg(script), reader).unwrap());
        (writer, handle)
    }

    fn signals(root: &Path) -> String {
        std::fs::read_to_string(root.join("signals")).unwrap_or_default()
    }

    fn wait_for(path: &Path) {
        for _ in 0..200 {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{} never appeared", path.display());
    }

    #[test]
    fn a_closed_lifeline_interrupts_the_run_as_stop_would() {
        let root = root();
        let (writer, handle) = supervised(child_script(&root, true));
        wait_for(&root.join("started"));
        drop(writer);
        let status = handle.join().unwrap();
        assert!(status.success(), "{status:?}");
        assert_eq!(signals(&root), "int\n");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_requested_interrupt_reaches_the_run_and_the_lifeline_stays_open() {
        let root = root();
        let (mut writer, handle) = supervised(child_script(&root, true));
        wait_for(&root.join("started"));
        writer.write_all(&[INTERRUPT]).unwrap();
        assert!(handle.join().unwrap().success());
        assert_eq!(signals(&root), "int\n");
        drop(writer);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_run_that_ignores_the_interrupt_is_terminated_then_killed() {
        let root = root();
        let (writer, handle) = supervised(child_script(&root, false));
        wait_for(&root.join("started"));
        let started = Instant::now();
        drop(writer);
        let status = handle.join().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
        assert_eq!(signals(&root), "int\nterm\n");
        assert!(started.elapsed() >= INTERRUPT_GRACE + TERMINATE_GRACE);
        let _ = std::fs::remove_dir_all(root);
    }
}
