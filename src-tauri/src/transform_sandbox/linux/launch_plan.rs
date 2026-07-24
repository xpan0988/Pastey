//! Closed Bubblewrap launch plan assembled only from verified Host-owned parts.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, PermissionsExt},
};

use crate::transform_sandbox::staging::StagedSnapshot;

use super::cgroup::ResourcePolicy;

#[derive(Clone, Debug)]
pub(crate) struct VerifiedExecutable {
    path: PathBuf,
    digest: String,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl VerifiedExecutable {
    pub(crate) fn verify(path: &Path, expected_name: &str) -> io::Result<Self> {
        if path.file_name().and_then(|value| value.to_str()) != Some(expected_name) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unverified executable shape",
            ));
        }
        let (canonical, metadata) = inspect_executable(path)?;
        Ok(Self {
            path: canonical,
            digest: blake3::hash(&fs::read(path)?).to_hex().to_string(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    pub(crate) fn reverify(&self) -> io::Result<()> {
        let (canonical, metadata) = inspect_executable(&self.path)?;
        if canonical != self.path {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "executable path identity changed",
            ));
        }
        #[cfg(unix)]
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "executable file identity changed",
            ));
        }
        let current = blake3::hash(&fs::read(&self.path)?).to_hex().to_string();
        if current != self.digest {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "executable identity changed",
            ));
        }
        Ok(())
    }

    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn inspect_executable(path: &Path) -> io::Result<(PathBuf, fs::Metadata)> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unverified executable shape",
        ));
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode();
        let uid = metadata.uid();
        if mode & 0o111 == 0 || mode & 0o022 != 0 || (uid != 0 && uid != unsafe { libc::geteuid() })
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unverified executable ownership or mode",
            ));
        }
    }
    let canonical = fs::canonicalize(path)?;
    if canonical != path {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "executable path is not canonical",
        ));
    }
    Ok((canonical, metadata))
}

#[derive(Debug)]
pub(crate) struct VerifiedSeccompPolicy {
    file: File,
    digest: String,
}

impl VerifiedSeccompPolicy {
    pub(crate) fn fixed(file: File, expected_digest: String) -> io::Result<Self> {
        let bytes = read_file_from_start(&file)?;
        let digest = blake3::hash(&bytes).to_hex().to_string();
        if digest != expected_digest || bytes.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "seccomp policy identity mismatch",
            ));
        }
        Ok(Self { file, digest })
    }
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
    #[cfg(unix)]
    pub(crate) fn raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
    }
}

#[derive(Debug)]
pub(crate) struct BubblewrapLaunchPlan {
    bubblewrap: VerifiedExecutable,
    supervisor: VerifiedExecutable,
    worker: VerifiedExecutable,
    staged_input: StagedSnapshot,
    seccomp_policy: VerifiedSeccompPolicy,
    resource_policy: ResourcePolicy,
    ready_fd: i32,
    release_fd: i32,
}

impl BubblewrapLaunchPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        bubblewrap: VerifiedExecutable,
        supervisor: VerifiedExecutable,
        worker: VerifiedExecutable,
        staged_input: StagedSnapshot,
        seccomp_policy: VerifiedSeccompPolicy,
        resource_policy: ResourcePolicy,
        ready_fd: i32,
        release_fd: i32,
    ) -> io::Result<Self> {
        for executable in [&bubblewrap, &supervisor, &worker] {
            executable.reverify()?;
        }
        if staged_input
            .input_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("artifact")
            || staged_input.work_dir.parent() != Some(staged_input.root.as_path())
            || ready_fd < 3
            || release_fd < 3
            || ready_fd == release_fd
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid closed launch input",
            ));
        }
        Ok(Self {
            bubblewrap,
            supervisor,
            worker,
            staged_input,
            seccomp_policy,
            resource_policy,
            ready_fd,
            release_fd,
        })
    }

    pub(crate) fn command(&self) -> io::Result<Command> {
        for executable in [&self.bubblewrap, &self.supervisor, &self.worker] {
            executable.reverify()?;
        }
        #[cfg(unix)]
        if unsafe { libc::fcntl(self.seccomp_policy.file.as_raw_fd(), libc::F_SETFD, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut command = Command::new(self.bubblewrap.path());
        command.env_clear().current_dir("/");
        for argument in self.render_arguments() {
            command.arg(argument);
        }
        Ok(command)
    }

    pub(crate) fn worker_digest(&self) -> &str {
        self.worker.digest()
    }
    pub(crate) fn seccomp_digest(&self) -> &str {
        self.seccomp_policy.digest()
    }
    pub(crate) fn resource_policy(&self) -> ResourcePolicy {
        self.resource_policy
    }

    fn render_arguments(&self) -> Vec<String> {
        vec![
            "--die-with-parent".into(),
            "--new-session".into(),
            "--unshare-user".into(),
            "--unshare-pid".into(),
            "--unshare-net".into(),
            "--unshare-ipc".into(),
            "--clearenv".into(),
            "--dir".into(),
            "/input".into(),
            "--ro-bind".into(),
            self.staged_input.input_path.to_string_lossy().into_owned(),
            "/input/artifact".into(),
            "--bind".into(),
            self.staged_input.work_dir.to_string_lossy().into_owned(),
            "/work".into(),
            "--ro-bind".into(),
            self.supervisor.path().to_string_lossy().into_owned(),
            "/pastey-supervisor".into(),
            "--ro-bind".into(),
            self.worker.path().to_string_lossy().into_owned(),
            "/pastey-worker".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--chdir".into(),
            "/work".into(),
            "--seccomp".into(),
            self.seccomp_policy.file.as_raw_fd().to_string(),
            "--preserve-fds".into(),
            "2".into(),
            "--setenv".into(),
            "LANG".into(),
            "C.UTF-8".into(),
            "--".into(),
            "/pastey-supervisor".into(),
            "--worker".into(),
            "/pastey-worker".into(),
            "--ready-fd".into(),
            self.ready_fd.to_string(),
            "--release-fd".into(),
            self.release_fd.to_string(),
        ]
    }
}

fn read_file_from_start(file: &File) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(bytes)
}

pub(crate) fn fixed_seccomp_policy_file(parent: &Path) -> io::Result<(File, String)> {
    let path = parent.join(format!("seccomp-policy-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let policy = fixed_seccomp_bpf();
    file.write_all(&policy)?;
    file.sync_all()?;
    fs::remove_file(path)?;
    let digest = blake3::hash(&policy).to_hex().to_string();
    Ok((file, digest))
}

#[cfg(target_os = "linux")]
fn fixed_seccomp_bpf() -> Vec<u8> {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Filter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }
    const LD_W_ABS: u16 = 0x20;
    const JMP_JEQ: u16 = 0x15;
    const RET: u16 = 0x06;
    const ERRNO: u32 = 0x0005_0000 | libc::EPERM as u32;
    const ALLOW: u32 = 0x7fff_0000;
    const KILL_PROCESS: u32 = 0x8000_0000;
    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xc000_00b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const AUDIT_ARCH: u32 = 0;
    let mut filters = vec![
        Filter {
            // seccomp_data.arch
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 4,
        },
        Filter {
            code: JMP_JEQ,
            jt: 1,
            jf: 0,
            k: AUDIT_ARCH,
        },
        Filter {
            code: RET,
            jt: 0,
            jf: 0,
            k: KILL_PROCESS,
        },
        Filter {
            // seccomp_data.nr
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        },
    ];
    for syscall in [
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_ptrace,
        libc::SYS_bpf,
    ] {
        filters.push(Filter {
            code: JMP_JEQ,
            jt: 0,
            jf: 1,
            k: syscall as u32,
        });
        filters.push(Filter {
            code: RET,
            jt: 0,
            jf: 0,
            k: ERRNO,
        });
    }
    filters.push(Filter {
        code: RET,
        jt: 0,
        jf: 0,
        k: ALLOW,
    });
    let byte_len = filters.len() * std::mem::size_of::<Filter>();
    unsafe { std::slice::from_raw_parts(filters.as_ptr().cast::<u8>(), byte_len).to_vec() }
}

#[cfg(not(target_os = "linux"))]
fn fixed_seccomp_bpf() -> Vec<u8> {
    b"non-linux-unavailable-policy".to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_verification_rejects_alias_and_identity_change() {
        let root =
            std::env::temp_dir().join(format!("pastey-executable-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let path = root.join("pastey-worker");
        fs::write(&path, b"fixed-worker").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let verified = VerifiedExecutable::verify(&path, "pastey-worker").unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(verified.reverify().is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"fixed-worker").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(verified.reverify().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn executable_reverification_rejects_mode_and_symlink_changes() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};

        let root =
            std::env::temp_dir().join(format!("pastey-executable-mode-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let path = root.join("pastey-worker");
        fs::write(&path, b"fixed-worker").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let verified = VerifiedExecutable::verify(&path, "pastey-worker").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o722)).unwrap();
        assert!(verified.reverify().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("same-bytes");
        fs::write(&target, b"fixed-worker").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(verified.reverify().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixed_seccomp_policy_is_digest_bound() {
        let root =
            std::env::temp_dir().join(format!("pastey-seccomp-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let (file, digest) = fixed_seccomp_policy_file(&root).unwrap();
        assert_eq!(
            VerifiedSeccompPolicy::fixed(file, digest)
                .unwrap()
                .digest()
                .len(),
            64
        );
        fs::remove_dir_all(root).unwrap();
    }
}
