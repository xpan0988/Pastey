//! The containment of a Claude Code run (Windows): the counterpart of
//! `lifeline`.
//!
//! `claude` starts suspended, joins a Job Object of its own, and only then
//! runs, so nothing it starts is ever outside the job. The job kills every
//! process in it when its last handle closes. Pastey holds the only handle
//! and it is never inheritable, so however Pastey's process ends (a crash, a
//! forced quit, `TerminateProcess`), Windows ends the run and every process
//! it started.
//!
//! The job permits no breakaway. A descendant that asks to leave it is
//! refused, and one started from a nested job that allows silent breakaway
//! (as libuv's own job does) stays in this one: breakaway stops at the first
//! job in the chain that does not permit it.
//!
//! Nothing can leave the job, but a process can ask a broker outside it to
//! start another (WMI, the Task Scheduler, a service). Such a process is not
//! the run's and is not contained, as one that left its process group is not
//! on macOS.
//!
//! A termination counts only once the job reports no process left.

use std::{
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::System::{
    JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
    Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED},
};

#[link(name = "ntdll")]
extern "system" {
    /// Resumes every thread of a process started suspended.
    fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

/// The exit code of a process the job terminated.
const TERMINATED_EXIT_CODE: u32 = 1;
const POLL: Duration = Duration::from_millis(20);

/// A kill-on-close Job Object that permits no breakaway.
pub(in crate::native_agent) struct ContainedJobV1 {
    handle: OwnedHandle,
}

impl ContainedJobV1 {
    pub(in crate::native_agent) fn create() -> io::Result<Self> {
        // SAFETY: no security attributes (the handle is not inheritable) and
        // no name (no other process can open it).
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new handle this process owns.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: a zeroed structure is valid; only the limit flags are set.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // Neither JOB_OBJECT_LIMIT_BREAKAWAY_OK nor _SILENT_BREAKAWAY_OK.
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job handle is valid and the size is the structure's.
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    /// Starts `command` inside the job, without a console window. It runs
    /// only once it is in the job; if it cannot join, it is ended before it
    /// ran anything, and the error says so.
    pub(in crate::native_agent) fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut child = command
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()?;
        // SAFETY: both handles are valid; the child's is not yet reaped.
        let assigned =
            unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), child.as_raw_handle()) };
        if assigned == 0 {
            let error = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "the process could not join its Job Object ({error}); it was ended before it ran"
            )));
        }
        // SAFETY: a valid process handle of a child started suspended.
        let status = unsafe { NtResumeProcess(child.as_raw_handle()) };
        if status < 0 {
            let _ = self.terminate();
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "the contained process could not be resumed (NTSTATUS {status:#x})"
            )));
        }
        Ok(child)
    }

    /// How many processes the job still holds.
    pub(in crate::native_agent) fn active_processes(&self) -> io::Result<u32> {
        // SAFETY: a zeroed structure is valid output space.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the job handle is valid and the size is the structure's.
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                std::ptr::addr_of_mut!(info).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }

    /// Ends every process in the job (the SIGKILL of a process group).
    pub(in crate::native_agent) fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job handle is valid.
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), TERMINATED_EXIT_CODE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits until the job holds no process, at most `timeout`. `false`
    /// when it still holds one, or cannot say.
    pub(in crate::native_agent) fn wait_empty(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.active_processes() {
                Ok(0) => return true,
                Ok(_) if Instant::now() < deadline => thread::sleep(POLL),
                _ => return false,
            }
        }
    }

    /// Ends whatever the job still holds and confirms it: `Ok(n)` when `n`
    /// processes were left and are now gone, `Err` when the job could not
    /// be shown empty within `timeout`.
    pub(in crate::native_agent) fn end_remaining(&self, timeout: Duration) -> Result<u32, String> {
        let left = self
            .active_processes()
            .map_err(|error| format!("the Job Object could not be read ({error})"))?;
        if left == 0 {
            return Ok(0);
        }
        if let Err(error) = self.terminate() {
            return Err(format!("the Job Object could not be terminated ({error})"));
        }
        if self.wait_empty(timeout) {
            Ok(left)
        } else {
            Err(format!(
                "{} process(es) were still running after termination",
                self.active_processes().unwrap_or(left)
            ))
        }
    }

    /// The ids of the processes the job holds (tests).
    #[cfg(test)]
    pub(in crate::native_agent) fn process_ids(&self) -> Vec<u32> {
        use windows_sys::Win32::System::JobObjects::JobObjectBasicProcessIdList;
        #[repr(C)]
        struct List {
            assigned: u32,
            listed: u32,
            ids: [usize; 256],
        }
        // SAFETY: a zeroed structure is valid output space.
        let mut list: List = unsafe { std::mem::zeroed() };
        // SAFETY: the job handle is valid and the size is the structure's.
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectBasicProcessIdList,
                std::ptr::addr_of_mut!(list).cast(),
                std::mem::size_of::<List>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(queried, 0, "{}", io::Error::last_os_error());
        list.ids[..list.listed as usize]
            .iter()
            .map(|id| *id as u32)
            .collect()
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn raw(&self) -> std::os::windows::io::RawHandle {
        self.handle.as_raw_handle()
    }
}

/// Runs `command` to its end inside a Job Object of its own (a short probe
/// such as `claude --version`), without a console window. However the probe
/// ends, nothing it started outlives the returned job.
pub(in crate::native_agent) fn spawn_probe(
    command: &mut Command,
) -> io::Result<(Child, ContainedJobV1)> {
    let job = ContainedJobV1::create()?;
    let child = job.spawn(command)?;
    Ok((child, job))
}

#[cfg(test)]
pub(in crate::native_agent) mod tests;
