//! Feature-gated deterministic probe for Linux isolation verification only.
//! It accepts no user data and is never part of the normal Pastey bundle.

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
        Mode::AttemptNetworkConnect => denied(connects(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)))),
        Mode::AttemptLoopbackConnect => denied(connects(IpAddr::V4(Ipv4Addr::LOCALHOST))),
        Mode::AttemptForbiddenSyscall => forbidden_syscall(),
        Mode::SpawnChild => spawn_descendants(false),
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
    emit("P100")
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

#[cfg(target_os = "linux")]
fn forbidden_syscall() -> i32 {
    if install_test_seccomp_policy().is_err() {
        return 2;
    }
    let result = unsafe { libc::getppid() };
    if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
        emit("P102")
    } else {
        1
    }
}

#[cfg(not(target_os = "linux"))]
fn forbidden_syscall() -> i32 {
    2
}

/// The test-only policy returns EPERM for getppid. ERRNO is selected over kill
/// or trap so the harness can deterministically observe denial and cleanup.
#[cfg(target_os = "linux")]
fn install_test_seccomp_policy() -> Result<(), ()> {
    #[repr(C)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }
    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;
    const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    let filters = [
        SockFilter {
            code: BPF_LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        },
        SockFilter {
            code: BPF_JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: libc::SYS_getppid as u32,
        },
        SockFilter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | libc::EPERM as u32,
        },
        SockFilter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
    ];
    let program = SockFprog {
        len: filters.len() as u16,
        filter: filters.as_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(());
    }
    if unsafe { libc::prctl(libc::PR_SET_SECCOMP, SECCOMP_SET_MODE_FILTER, &program) } != 0 {
        return Err(());
    }
    Ok(())
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
