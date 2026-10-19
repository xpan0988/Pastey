//! Codex's per-thread writer lock.
//!
//! Since 0.163, a Codex app-server that loads a thread holds an exclusive
//! lock on `<CODEX_HOME>/thread-writer-locks/<thread-id>.lock`, and removes
//! the file when it exits. Another app-server's `thread/resume` of that
//! thread is refused ("already has an active writer"). Measured on macOS
//! with the CLI and the desktop app's bundled Codex: the lock is a BSD
//! `flock`, which a POSIX `F_GETLK` test reports without acquiring anything
//! (`tools/codex-session-probe/writer_lock_probe.py`).
//!
//! Pastey only *reads* this lock, to say whether another Codex client (the
//! Codex app or a terminal) has a thread open. It never takes or removes it:
//! continuing a thread relies on Codex's own refusal, which holds whatever
//! this observation says.

use std::path::{Path, PathBuf};

/// Whether another Codex client has a thread open for writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WriterLockV1 {
    /// A process holds the thread's writer lock now.
    Held,
    /// No process holds it.
    Free,
    /// It cannot be read here without taking it.
    Unknown(&'static str),
}

const LOCK_DIR: &str = "thread-writer-locks";

/// The Codex home every Codex client on this Host shares.
pub(crate) fn codex_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".codex"))
}

/// One thread's lock, read without taking it.
pub(crate) fn thread_writer_lock(thread_id: &str) -> WriterLockV1 {
    match codex_home() {
        Some(home) => observe(&home, thread_id),
        None => WriterLockV1::Unknown("Codex's home folder is unknown"),
    }
}

pub(crate) fn observe(codex_home: &Path, thread_id: &str) -> WriterLockV1 {
    if thread_id.is_empty()
        || thread_id.len() > 64
        || !thread_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return WriterLockV1::Unknown("the thread id is not a Codex thread id");
    }
    let directory = codex_home.join(LOCK_DIR);
    if !directory.is_dir() {
        // An older Codex keeps no writer locks: no lock file proves nothing.
        return WriterLockV1::Unknown("this Codex keeps no thread writer locks");
    }
    let path = directory.join(format!("{thread_id}.lock"));
    if !path.exists() {
        // A lock cannot be held on a file that does not exist.
        return WriterLockV1::Free;
    }
    held(&path)
}

#[cfg(target_os = "macos")]
fn held(path: &Path) -> WriterLockV1 {
    use std::os::unix::io::AsRawFd;
    let Ok(file) = std::fs::File::open(path) else {
        return WriterLockV1::Unknown("the lock file could not be opened for reading");
    };
    // SAFETY: a zeroed `flock` is a valid request once its fields are set,
    // and F_GETLK only writes into it. It acquires nothing.
    let mut request: libc::flock = unsafe { std::mem::zeroed() };
    request.l_type = libc::F_WRLCK as libc::c_short;
    request.l_whence = libc::SEEK_SET as libc::c_short;
    request.l_start = 0;
    request.l_len = 0;
    // SAFETY: a valid descriptor and a valid `flock` for F_GETLK.
    let status = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut request) };
    if status != 0 {
        return WriterLockV1::Unknown("the lock could not be tested");
    }
    // macOS reports a BSD flock held elsewhere as a conflicting lock.
    if request.l_type == libc::F_UNLCK as libc::c_short {
        WriterLockV1::Free
    } else {
        WriterLockV1::Held
    }
}

#[cfg(not(target_os = "macos"))]
fn held(_path: &Path) -> WriterLockV1 {
    // Linux keeps flock locks apart from F_GETLK, and Windows has no test
    // that acquires nothing. A remaining file may be held or left behind.
    WriterLockV1::Unknown("Codex's lock on this platform cannot be read without taking it")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let root = std::env::temp_dir().join(format!("pastey-codex-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(LOCK_DIR)).unwrap();
        root
    }

    #[test]
    fn no_lock_file_is_free_and_no_lock_directory_is_unknown() {
        let root = home();
        assert_eq!(
            observe(&root, "01a140f0-6af4-7ef3-b799-dcd84c0275a6"),
            WriterLockV1::Free
        );
        let bare =
            std::env::temp_dir().join(format!("pastey-codex-nolocks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&bare).unwrap();
        assert!(matches!(
            observe(&bare, "01a140f0-6af4-7ef3-b799-dcd84c0275a6"),
            WriterLockV1::Unknown(_)
        ));
        assert!(matches!(
            observe(&root, "../escape"),
            WriterLockV1::Unknown(_)
        ));
    }

    /// A lock held by another process is reported, and a file left behind
    /// with no holder is free. The holder is a separate `perl` process,
    /// because F_GETLK never reports a lock of the asking process.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_flock_held_by_another_process_is_seen_without_taking_it() {
        use std::io::{BufRead, BufReader};
        let root = home();
        let id = "01a140f0-6af4-7ef3-b799-dcd84c0275a6";
        let path = root.join(LOCK_DIR).join(format!("{id}.lock"));
        std::fs::write(&path, b"").unwrap();
        assert_eq!(observe(&root, id), WriterLockV1::Free);
        let mut holder = std::process::Command::new("/usr/bin/perl")
            .args([
                "-e",
                "use Fcntl qw(:flock); open(my $f, '<', $ARGV[0]) or die; flock($f, LOCK_EX) or die; $| = 1; print \"held\\n\"; sleep 30;",
            ])
            .arg(&path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(holder.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "held");
        assert_eq!(observe(&root, id), WriterLockV1::Held);
        holder.kill().unwrap();
        holder.wait().unwrap();
        assert_eq!(observe(&root, id), WriterLockV1::Free);
    }
}
