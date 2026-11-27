//! One copy of the desktop app per data folder.
//!
//! Startup recovery treats whatever an earlier run left behind as abandoned:
//! it deletes partial files and marks transfers and Plan attempts
//! interrupted. A second copy started on the same data would do that to the
//! copy still running, so the app refuses to start instead.
//!
//! Two checks, both before any data is touched:
//! - A lock on a file in the data folder, held for as long as the app runs
//!   and released by the system however the app ends. Builds from before the
//!   rename to Maris never take it.
//! - The run marker those builds do write (Pastey 2.0 onwards): if it names a
//!   process that is still running this app's executable and that process
//!   did not hold the lock, an old Pastey is using the data.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use crate::logging;

const LOCK_FILE_NAME: &str = "pastey.lock";
/// The executable's name, which the rename to Maris kept.
const APP_EXECUTABLE_STEM: &str = "pastey";

/// Held for the life of the process; dropping it releases the data folder.
pub struct DataFolderClaim {
    _lock: File,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartRefusal {
    /// Another Maris holds this data folder.
    AlreadyRunning,
    /// A Pastey from before the rename is running on this data folder.
    LegacyRunning { pid: u32, version: Option<String> },
}

impl StartRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::AlreadyRunning => "Maris is already running on this computer. Open it from \
                its icon in the menu bar or system tray. Only one copy of Maris can use this \
                device's data at a time."
                .into(),
            Self::LegacyRunning { version, .. } => {
                let name = match version {
                    Some(version) => format!("Pastey {version}"),
                    None => "Pastey".into(),
                };
                format!(
                    "{name} is still running. Maris uses the same data as Pastey, and running \
                     both would interrupt transfers and Agent work in progress. Quit Pastey \
                     from its icon in the menu bar or system tray, then open Maris again. \
                     Once Maris is running, you can delete the old Pastey app."
                )
            }
        }
    }

    pub fn log_code(&self) -> &'static str {
        match self {
            Self::AlreadyRunning => "data_folder_in_use",
            Self::LegacyRunning { .. } => "legacy_pastey_running",
        }
    }
}

/// Claims the data folder for this process, or says why the app must not
/// start. A lock that cannot be taken for any reason other than another
/// holder (a file system without locks) does not stop the app.
pub fn claim(
    app_data_dir: &Path,
    logs_dir: &Path,
) -> Result<Option<DataFolderClaim>, StartRefusal> {
    let lock = match lock_data_folder(app_data_dir) {
        Ok(lock) => Some(lock),
        Err(LockOutcome::Held) => return Err(StartRefusal::AlreadyRunning),
        Err(LockOutcome::Unavailable(error)) => {
            eprintln!(
                "pastey: could not lock the data folder ({error}); starting without the check"
            );
            None
        }
    };
    if let Some(refusal) = legacy_copy_running(logs_dir) {
        return Err(refusal);
    }
    Ok(lock.map(|lock| DataFolderClaim { _lock: lock }))
}

enum LockOutcome {
    Held,
    Unavailable(std::io::Error),
}

fn lock_data_folder(app_data_dir: &Path) -> Result<File, LockOutcome> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(app_data_dir.join(LOCK_FILE_NAME))
        .map_err(LockOutcome::Unavailable)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(LockOutcome::Held),
        Err(TryLockError::Error(error)) => Err(LockOutcome::Unavailable(error)),
    }
}

/// Called only while holding the lock, so a live process in the run marker
/// is one that never took it.
fn legacy_copy_running(logs_dir: &Path) -> Option<StartRefusal> {
    let marker = fs::read_to_string(logging::run_marker_path(logs_dir)).ok()?;
    let (pid, version) = parse_run_marker(&marker)?;
    if pid == std::process::id() {
        return None;
    }
    // The pid of an app that crashed may since belong to something else.
    let executable = process_executable(pid)?;
    is_app_executable(&executable).then_some(StartRefusal::LegacyRunning { pid, version })
}

fn parse_run_marker(marker: &str) -> Option<(u32, Option<String>)> {
    let mut pid = None;
    let mut version = None;
    for field in marker.split_whitespace() {
        if let Some(value) = field.strip_prefix("pid=") {
            pid = value.parse().ok();
        } else if let Some(value) = field.strip_prefix("version=") {
            version = (!value.is_empty()).then(|| value.to_owned());
        }
    }
    Some((pid?, version))
}

fn is_app_executable(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case(APP_EXECUTABLE_STEM))
}

/// The executable of a running process, or `None` when there is no such
/// process (or it cannot be inspected).
#[cfg(target_os = "macos")]
fn process_executable(pid: u32) -> Option<PathBuf> {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(OsStr::from_bytes(&buffer)))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_executable(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(windows)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain process queries on a handle closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut exit_code = 0u32;
        let running = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        let named = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        ) != 0;
        CloseHandle(handle);
        if !running || !named {
            return None;
        }
        buffer.truncate(length as usize);
        Some(PathBuf::from(OsString::from_wide(&buffer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pastey-single-instance-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(dir.join("logs")).unwrap();
        dir
    }

    #[test]
    fn a_second_claim_on_the_same_data_folder_is_refused_until_the_first_ends() {
        let dir = scratch_dir("lock");
        let first = claim(&dir, &dir.join("logs")).unwrap();
        assert!(first.is_some());
        assert_eq!(
            claim(&dir, &dir.join("logs")).err(),
            Some(StartRefusal::AlreadyRunning)
        );
        drop(first);
        assert!(claim(&dir, &dir.join("logs")).unwrap().is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_data_folder_is_claimed_independently() {
        let one = scratch_dir("one");
        let two = scratch_dir("two");
        let _first = claim(&one, &one.join("logs")).unwrap();
        assert!(claim(&two, &two.join("logs")).unwrap().is_some());
        fs::remove_dir_all(one).unwrap();
        fs::remove_dir_all(two).unwrap();
    }

    #[test]
    fn the_run_marker_pastey_2_0_writes_is_read() {
        assert_eq!(
            parse_run_marker("started_at=1764000000 pid=4242 version=2.0.0"),
            Some((4242, Some("2.0.0".into())))
        );
        assert_eq!(parse_run_marker("started_at=1 pid=7"), Some((7, None)));
        assert_eq!(parse_run_marker("started_at=1 version=2.0.0"), None);
        assert_eq!(parse_run_marker(""), None);
    }

    #[test]
    fn only_this_apps_executable_counts_as_a_running_copy() {
        assert!(is_app_executable(Path::new(
            "/Applications/pastey.app/Contents/MacOS/pastey"
        )));
        assert!(is_app_executable(Path::new(
            "C:/Users/a/AppData/Local/pastey/pastey.exe"
        )));
        assert!(is_app_executable(Path::new(
            "C:/Program Files/Maris/PASTEY.EXE"
        )));
        assert!(!is_app_executable(Path::new("/usr/bin/pasteyd")));
        assert!(!is_app_executable(Path::new("/usr/bin/zsh")));
    }

    #[test]
    fn a_running_process_is_found_by_its_pid_and_a_missing_one_is_not() {
        let own = process_executable(std::process::id()).expect("own executable");
        assert_eq!(
            fs::canonicalize(own).unwrap(),
            fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
        );
        assert_eq!(process_executable(u32::MAX - 1), None);
    }

    #[test]
    fn a_run_marker_left_by_this_process_or_a_finished_one_does_not_block_the_start() {
        let dir = scratch_dir("marker");
        let logs = dir.join("logs");
        fs::write(
            logging::run_marker_path(&logs),
            format!("started_at=1 pid={} version=2.0.0", std::process::id()),
        )
        .unwrap();
        assert!(claim(&dir, &logs).is_ok());
        fs::write(
            logging::run_marker_path(&logs),
            format!("started_at=1 pid={} version=2.0.0", u32::MAX - 1),
        )
        .unwrap();
        assert!(claim(&dir, &logs).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_running_pastey_named_in_the_run_marker_blocks_the_start() {
        let dir = scratch_dir("legacy");
        let logs = dir.join("logs");
        // A stand-in for an old Pastey: a running process whose executable
        // is named like the app's.
        let stand_in = dir.join("pastey");
        fs::copy("/bin/sleep", &stand_in).unwrap();
        let mut child = std::process::Command::new(&stand_in)
            .arg("30")
            .spawn()
            .unwrap();
        fs::write(
            logging::run_marker_path(&logs),
            format!("started_at=1 pid={} version=2.0.0", child.id()),
        )
        .unwrap();
        let refusal = claim(&dir, &logs).err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            refusal,
            Some(StartRefusal::LegacyRunning {
                pid: child.id(),
                version: Some("2.0.0".into())
            })
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
