//! Physical Windows checks of the Job Object containment, on real processes
//! (`cmd.exe`, `ping.exe`, and this test binary run again as a helper
//! process). No Claude Code is involved: these show what the job itself
//! guarantees.

use super::*;
use std::io::{BufRead, BufReader};
use std::os::windows::io::RawHandle;
use std::process::Stdio;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        JobObjects::{
            IsProcessInJob, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        },
        Threading::{
            GetCurrentProcess, OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
    },
};

/// A handle that observes one process's end, opened while the process is
/// known to run, so a reused id never answers for it.
struct Watched(HANDLE);

impl Watched {
    fn open(pid: u32) -> Self {
        // SAFETY: plain OpenProcess; the handle is closed on drop.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(!handle.is_null(), "process {pid} is not running");
        Self(handle)
    }

    fn ended_within(&self, timeout: Duration) -> bool {
        // SAFETY: a valid process handle.
        unsafe { WaitForSingleObject(self.0, timeout.as_millis() as u32) == WAIT_OBJECT_0 }
    }

    fn in_job(&self, job: RawHandle) -> bool {
        let mut result = 0;
        // SAFETY: valid process and job handles.
        assert_ne!(unsafe { IsProcessInJob(self.0, job, &mut result) }, 0);
        result != 0
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        // SAFETY: a handle this test opened.
        unsafe { CloseHandle(self.0) };
    }
}

/// `cmd.exe` running one `ping` in the background and one in the
/// foreground: a root with two descendants, each about ten minutes long.
fn tree_command() -> Command {
    let mut command = Command::new("cmd.exe");
    command
        .args([
            "/d",
            "/c",
            "start /b ping -n 600 127.0.0.1 >nul & ping -n 600 127.0.0.1 >nul",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The image name (`ping.exe`) of a running process, if it can be read.
fn image_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::QueryFullProcessImageNameW;
    // SAFETY: plain OpenProcess; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: a valid handle and a buffer of `length` characters.
    let read = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) };
    // SAFETY: a handle opened above.
    unsafe { CloseHandle(handle) };
    (read != 0).then(|| {
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit('\\').next().unwrap_or("").to_ascii_lowercase()
    })
}

/// The job's processes by image name, once it holds at least `wanted`
/// (Windows adds the hidden console's `conhost.exe`, also inside the job).
fn wait_for_images(job: &ContainedJobV1, wanted: &[&str]) -> Vec<(u32, String)> {
    for _ in 0..500 {
        let found: Vec<_> = job
            .process_ids()
            .into_iter()
            .filter_map(|id| Some((id, image_name(id)?)))
            .collect();
        let holds = |name: &str| found.iter().filter(|(_, image)| image == name).count();
        if wanted
            .iter()
            .all(|name| holds(name) >= wanted.iter().filter(|other| *other == name).count())
        {
            return found;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the job never held {wanted:?}: {:?}", job.process_ids());
}

#[test]
fn a_contained_tree_starts_inside_the_job_and_ends_with_its_termination() {
    let job = ContainedJobV1::create().unwrap();
    let mut root = job.spawn(&mut tree_command()).unwrap();
    let found = wait_for_images(&job, &["cmd.exe", "ping.exe", "ping.exe"]);
    eprintln!("contained: {found:?}");
    assert!(found.iter().any(|(id, _)| *id == root.id()), "{found:?}");
    let watched: Vec<_> = found.iter().map(|(id, _)| Watched::open(*id)).collect();
    assert!(watched.iter().all(|process| process.in_job(job.raw())));

    let ended = job.end_remaining(Duration::from_secs(5)).unwrap();
    assert!(ended as usize >= found.len(), "{ended} of {found:?}");
    assert!(watched
        .iter()
        .all(|process| process.ended_within(Duration::from_secs(2))));
    assert_eq!(job.active_processes().unwrap(), 0);
    assert_eq!(
        root.wait().unwrap().code(),
        Some(TERMINATED_EXIT_CODE as i32)
    );
    // An empty job has nothing left to end.
    assert_eq!(job.end_remaining(Duration::from_secs(1)), Ok(0));
}

#[test]
fn a_root_that_exits_leaves_its_descendants_in_the_job_until_they_are_ended() {
    let job = ContainedJobV1::create().unwrap();
    let mut command = Command::new("cmd.exe");
    command
        .args(["/d", "/c", "start /b ping -n 600 127.0.0.1 >nul & exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut root = job.spawn(&mut command).unwrap();
    assert!(root.wait().unwrap().success());
    // The background `ping` outlived its parent, still inside the job.
    let found = wait_for_images(&job, &["ping.exe"]);
    let ping = found
        .iter()
        .find(|(_, image)| image == "ping.exe")
        .unwrap()
        .0;
    let watched = Watched::open(ping);
    assert!(job.end_remaining(Duration::from_secs(5)).unwrap() >= 1);
    assert!(watched.ended_within(Duration::from_secs(2)));
}

#[test]
fn a_probe_runs_without_a_console_window_inside_its_own_job() {
    let mut command = Command::new("cmd.exe");
    command
        .args(["/d", "/c", "echo contained"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let (child, job) = spawn_probe(&mut command).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "contained");
    assert!(job.wait_empty(Duration::from_secs(2)));
}

/// Runs one of the helper tests below as a separate process inside or
/// outside `job`, and returns it with its stdout's lines.
fn helper(
    name: &str,
    job: Option<&ContainedJobV1>,
) -> (Child, std::io::Lines<BufReader<std::process::ChildStdout>>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            &format!("native_agent::capability::claude_code::job::tests::{name}"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PASTEY_JOB_HELPER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match job {
        Some(job) => job.spawn(&mut command).unwrap(),
        None => command.spawn().unwrap(),
    };
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    (child, lines)
}

/// The value the helper printed as `name=value` (libtest may print the
/// test's name before it on the same line).
fn marker(lines: &mut std::io::Lines<BufReader<std::process::ChildStdout>>, name: &str) -> String {
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some((_, value)) = line.split_once(&format!("{name}=")) {
            return value.trim().to_owned();
        }
    }
    panic!("the helper never printed {name}");
}

/// Inside the job: asks to break away, then joins a nested job that permits
/// silent breakaway (as libuv's own job does) and starts a child from it.
#[test]
#[ignore = "run only as a helper process by its parent test"]
fn breakaway_helper() {
    if std::env::var_os("PASTEY_JOB_HELPER").is_none() {
        return;
    }
    let explicit = Command::new("cmd.exe")
        .args(["/d", "/c", "exit 0"])
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW)
        .spawn();
    match explicit {
        Ok(mut child) => {
            let _ = child.wait();
            println!("EXPLICIT=allowed");
        }
        Err(error) => println!("EXPLICIT=refused {}", error.raw_os_error().unwrap_or(0)),
    }
    // SAFETY: a zeroed structure is valid; only the limit flags are set.
    let nested = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!nested.is_null());
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_BREAKAWAY_OK
        | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
    // SAFETY: valid handles and sizes.
    unsafe {
        assert_ne!(
            SetInformationJobObject(
                nested,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ),
            0
        );
        assert_ne!(AssignProcessToJobObject(nested, GetCurrentProcess()), 0);
    }
    let child = Command::new("ping.exe")
        .args(["-n", "600", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let watched = Watched::open(child.id());
    println!("SILENT_CHILD={}", child.id());
    println!("SILENT_CHILD_IN_NESTED={}", watched.in_job(nested));
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn nothing_started_inside_the_job_can_leave_it() {
    let job = ContainedJobV1::create().unwrap();
    let (mut helper, mut lines) = helper("breakaway_helper", Some(&job));
    // ERROR_ACCESS_DENIED: the job does not permit breakaway.
    assert_eq!(marker(&mut lines, "EXPLICIT"), "refused 5");
    let child: u32 = marker(&mut lines, "SILENT_CHILD").parse().unwrap();
    let watched = Watched::open(child);
    // It left the nested job silently, and stayed in this one.
    assert_eq!(marker(&mut lines, "SILENT_CHILD_IN_NESTED"), "false");
    assert!(watched.in_job(job.raw()));
    assert!(job.process_ids().contains(&child));

    assert!(job.end_remaining(Duration::from_secs(5)).is_ok());
    assert!(watched.ended_within(Duration::from_secs(2)));
    let _ = helper.wait();
}

/// Stands in for Pastey: contains a tree, says which processes it holds,
/// then waits to be killed.
#[test]
#[ignore = "run only as a helper process by its parent test"]
fn owner_helper() {
    if std::env::var_os("PASTEY_JOB_HELPER").is_none() {
        return;
    }
    let job = ContainedJobV1::create().unwrap();
    let _root = job.spawn(&mut tree_command()).unwrap();
    let found = wait_for_images(&job, &["cmd.exe", "ping.exe", "ping.exe"]);
    println!(
        "CONTAINED={}",
        found
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn when_the_owner_is_killed_windows_ends_everything_it_contained() {
    // Only so a failing test leaves nothing behind: this outer job is never
    // terminated while the check runs.
    let guard = ContainedJobV1::create().unwrap();
    let (mut owner, mut lines) = helper("owner_helper", Some(&guard));
    let contained: Vec<Watched> = marker(&mut lines, "CONTAINED")
        .split(',')
        .map(|id| Watched::open(id.parse().unwrap()))
        .collect();
    assert!(contained.len() >= 3);
    assert!(!contained[0].ended_within(Duration::from_millis(200)));
    // TerminateProcess: no handler, no cleanup, as a crash or forced quit.
    owner.kill().unwrap();
    owner.wait().unwrap();
    let killed = Instant::now();
    for process in &contained {
        assert!(
            process.ended_within(Duration::from_secs(2)),
            "a contained process outlived its owner"
        );
    }
    eprintln!(
        "contained tree ended {:?} after its owner was killed",
        killed.elapsed()
    );
}
