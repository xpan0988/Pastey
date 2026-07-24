//! Feature-gated deterministic probe for Linux isolation verification only.
//! It is compiled only with the verification feature, is not packaged, and
//! accepts no user data or Tauri, Bridge, candidate, or Transform authority.

#[cfg(not(feature = "transform-sandbox-verification"))]
compile_error!("the transform sandbox probe requires its verification feature");

use std::{
    fs,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

const ALLOWED_MARKER: &[u8] = b"pastey-verification-allowed-v1";
const FORBIDDEN_PATH: &str = "/fixture/forbidden/secret-marker";
const ALLOWED_PATH: &str = "/fixture/allowed/probe-input";
const WORK_PATH: &str = "/fixture/work/probe-output";

#[derive(Clone, Copy)]
enum Mode {
    ReportVisibleFixtures,
    AttemptForbiddenRead,
    AttemptForbiddenWrite,
    AttemptNetworkConnect,
    AttemptLoopbackConnect,
    AttemptForbiddenSyscall,
    SpawnChild,
    SpawnGrandchild,
    DoubleForkOrDaemonize,
    AllocateMemory,
    ConsumeCpu,
    FloodOutput,
    WaitForSignal,
}

impl Mode {
    fn parse(value: Option<&str>) -> Option<Self> {
        Some(match value? {
            "report-visible-fixtures" => Self::ReportVisibleFixtures,
            "attempt-forbidden-read" => Self::AttemptForbiddenRead,
            "attempt-forbidden-write" => Self::AttemptForbiddenWrite,
            "attempt-network-connect" => Self::AttemptNetworkConnect,
            "attempt-loopback-connect" => Self::AttemptLoopbackConnect,
            "attempt-forbidden-syscall" => Self::AttemptForbiddenSyscall,
            "spawn-child" => Self::SpawnChild,
            "spawn-grandchild" => Self::SpawnGrandchild,
            "double-fork-or-daemonize" => Self::DoubleForkOrDaemonize,
            "allocate-memory" => Self::AllocateMemory,
            "consume-cpu" => Self::ConsumeCpu,
            "flood-output" => Self::FloodOutput,
            "wait-for-signal" => Self::WaitForSignal,
            _ => return None,
        })
    }
}

fn main() {
    let mode = Mode::parse(std::env::args().nth(1).as_deref());
    let code = mode.map(run).unwrap_or(64);
    std::process::exit(code);
}

fn run(mode: Mode) -> i32 {
    match mode {
        Mode::ReportVisibleFixtures => report_visible_fixtures(),
        Mode::AttemptForbiddenRead => denied(fs::read(FORBIDDEN_PATH).is_err()),
        Mode::AttemptForbiddenWrite => {
            denied(fs::write("/fixture/allowed/forbidden-write", b"x").is_err())
        }
        Mode::AttemptNetworkConnect => denied(network_is_denied(connects(IpAddr::V4(
            Ipv4Addr::new(198, 18, 0, 1),
        )))),
        Mode::AttemptLoopbackConnect => {
            denied(network_is_denied(connects(IpAddr::V4(Ipv4Addr::LOCALHOST))))
        }
        Mode::AttemptForbiddenSyscall => forbidden_syscall(),
        Mode::SpawnChild => pressure_pids(),
        Mode::SpawnGrandchild => spawn_descendants(true),
        Mode::DoubleForkOrDaemonize => double_fork(),
        Mode::AllocateMemory => allocate_memory(),
        Mode::ConsumeCpu => consume_cpu(),
        Mode::FloodOutput => flood_output(),
        Mode::WaitForSignal => wait_for_signal(),
    }
}

fn report_visible_fixtures() -> i32 {
    if !matches!(fs::read(ALLOWED_PATH), Ok(bytes) if bytes == ALLOWED_MARKER)
        || fs::read(FORBIDDEN_PATH).is_ok()
    {
        return 1;
    }
    if fs::write(WORK_PATH, b"pastey-verification-work-v1").is_err() {
        return 2;
    }
    let pid_namespace = match fs::read_link("/proc/self/ns/pid") {
        Ok(value) => value,
        Err(_) => return 3,
    };
    let network_namespace = match fs::read_link("/proc/self/ns/net") {
        Ok(value) => value,
        Err(_) => return 4,
    };
    println!(
        "P100 pidns={} netns={}",
        pid_namespace.to_string_lossy(),
        network_namespace.to_string_lossy()
    );
    0
}

fn denied(value: bool) -> i32 {
    if value {
        emit("P101")
    } else {
        1
    }
}

fn connects(address: IpAddr) -> bool {
    TcpStream::connect_timeout(&SocketAddr::new(address, 9), Duration::from_millis(100)).is_ok()
}

fn network_is_denied(connected: bool) -> bool {
    !connected
}

#[cfg(target_os = "linux")]
fn forbidden_syscall() -> i32 {
    // The outer Bubblewrap policy used by production denies socket(2). Do not
    // install a second policy here: doing so would let this probe pass even if
    // Bubblewrap ignored or failed to apply the production policy.
    let result = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
        emit("P102")
    } else {
        if result >= 0 {
            unsafe { libc::close(result) };
        }
        1
    }
}

#[cfg(not(target_os = "linux"))]
fn forbidden_syscall() -> i32 {
    2
}

#[cfg(target_os = "linux")]
fn pressure_pids() -> i32 {
    for _ in 0..12 {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            emit("P103");
            return wait_for_signal();
        }
        if pid == 0 {
            return wait_for_signal();
        }
    }
    emit("P103");
    wait_for_signal()
}

#[cfg(not(target_os = "linux"))]
fn pressure_pids() -> i32 {
    2
}

#[cfg(target_os = "linux")]
fn spawn_descendants(grandchild: bool) -> i32 {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return 2;
    }
    if pid == 0 {
        if grandchild {
            let nested = unsafe { libc::fork() };
            if nested == 0 {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    emit("P103")
}

#[cfg(not(target_os = "linux"))]
fn spawn_descendants(_grandchild: bool) -> i32 {
    2
}

#[cfg(target_os = "linux")]
fn double_fork() -> i32 {
    let first = unsafe { libc::fork() };
    if first < 0 {
        return 2;
    }
    if first == 0 {
        if unsafe { libc::setsid() } < 0 {
            std::process::exit(2);
        }
        let second = unsafe { libc::fork() };
        if second == 0 {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        std::process::exit(0);
    }
    emit("P104")
}

#[cfg(not(target_os = "linux"))]
fn double_fork() -> i32 {
    2
}

fn allocate_memory() -> i32 {
    let mut bytes = Vec::with_capacity(64 * 1024 * 1024);
    for _ in 0..64 {
        bytes.extend_from_slice(&[0_u8; 1024 * 1024]);
    }
    std::hint::black_box(bytes.len());
    emit("P105")
}

fn consume_cpu() -> i32 {
    let mut value = 0_u64;
    for index in 0..20_000_000_u64 {
        value = value.wrapping_add(index.rotate_left((index % 31) as u32));
    }
    std::hint::black_box(value);
    emit("P106")
}

fn flood_output() -> i32 {
    let block = [b'x'; 1024];
    for _ in 0..1024 {
        if io::stdout().write_all(&block).is_err() {
            return 2;
        }
    }
    emit("P107")
}

fn wait_for_signal() -> i32 {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn emit(value: &str) -> i32 {
    println!("{value}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_observation_rejects_success_and_accepts_denial() {
        assert!(!network_is_denied(true));
        assert!(network_is_denied(false));
    }

    #[test]
    fn probe_does_not_install_a_second_seccomp_policy() {
        let source = include_str!("test_probe.rs");
        assert!(!source.contains(&["PR_SET", "SECCOMP"].join("_")));
        assert!(!source.contains(&["install_test", "seccomp_policy"].join("_")));
    }
}
