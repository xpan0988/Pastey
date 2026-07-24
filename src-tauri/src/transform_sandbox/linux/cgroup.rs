//! One-operation cgroup-v2 lifecycle. No parent mutation and no reuse.

use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::{
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResourcePolicy {
    pub(crate) memory_max: u64,
    pub(crate) pids_max: u32,
    pub(crate) cpu_quota: u32,
    pub(crate) cpu_period: u32,
}

pub(crate) const TEXT_TRANSFORM_RESOURCE_POLICY: ResourcePolicy = ResourcePolicy {
    memory_max: 64 * 1024 * 1024,
    pids_max: 8,
    cpu_quota: 50_000,
    cpu_period: 100_000,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResourceCounters {
    pub(crate) memory_limit_events: u64,
    pub(crate) pids_limit_events: u64,
    pub(crate) cpu_throttled_events: u64,
}

#[derive(Debug)]
pub(crate) struct CgroupOperation {
    parent: PathBuf,
    path: PathBuf,
    cleaned: bool,
}

impl CgroupOperation {
    #[cfg(target_os = "linux")]
    pub(crate) fn create(policy: ResourcePolicy) -> io::Result<Self> {
        let parent = discover_current_delegated_cgroup()?;
        Self::create_under(&parent, policy)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn create(_policy: ResourcePolicy) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cgroup v2 requires Linux",
        ))
    }

    fn create_under(parent: &Path, policy: ResourcePolicy) -> io::Result<Self> {
        verify_delegation(parent)?;
        let path = parent.join(format!("pastey-transform-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        let mut operation = Self {
            parent: parent.to_path_buf(),
            path,
            cleaned: false,
        };
        let configured = operation.configure(policy);
        if configured.is_err() {
            let _ = operation.cleanup();
        }
        configured.map(|_| operation)
    }

    fn configure(&mut self, policy: ResourcePolicy) -> io::Result<()> {
        write_required(
            &self.path.join("memory.max"),
            &policy.memory_max.to_string(),
        )?;
        write_required(&self.path.join("pids.max"), &policy.pids_max.to_string())?;
        write_required(
            &self.path.join("cpu.max"),
            &format!("{} {}", policy.cpu_quota, policy.cpu_period),
        )?;
        if !self.path.join("cgroup.procs").exists() || !self.path.join("cgroup.kill").exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private cgroup control files unavailable",
            ));
        }
        Ok(())
    }

    pub(crate) fn attach(&self, pid: u32) -> io::Result<()> {
        if self.cleaned || pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid cgroup attach",
            ));
        }
        write_required(&self.path.join("cgroup.procs"), &pid.to_string())
    }

    /// Moves the forked child into this private cgroup before `execve`. Writing
    /// `0` to cgroup.procs moves the writing process itself, so no PID
    /// formatting, allocation, filesystem lookup, or other non-async-signal-safe
    /// work occurs in the post-fork child.
    #[cfg(target_os = "linux")]
    pub(crate) fn configure_pre_exec_self_attach(&self, command: &mut Command) -> io::Result<()> {
        let cgroup_procs = fs::OpenOptions::new()
            .write(true)
            .open(self.path.join("cgroup.procs"))?;
        unsafe {
            command.pre_exec(move || {
                let value = b"0";
                let written = libc::write(
                    cgroup_procs.as_raw_fd(),
                    value.as_ptr().cast::<libc::c_void>(),
                    value.len(),
                );
                if written == value.len() as isize {
                    Ok(())
                } else if written < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "short cgroup self-attach write",
                    ))
                }
            });
        }
        Ok(())
    }

    pub(crate) fn process_ids(&self) -> io::Result<Vec<u32>> {
        let value = fs::read_to_string(self.path.join("cgroup.procs"))?;
        value
            .split_whitespace()
            .map(|pid| {
                pid.parse::<u32>()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid cgroup pid"))
            })
            .collect()
    }

    pub(crate) fn resource_counters(&self) -> io::Result<ResourceCounters> {
        let memory = read_named_counters(&self.path.join("memory.events"))?;
        let pids = read_named_counters(&self.path.join("pids.events"))?;
        let cpu = read_named_counters(&self.path.join("cpu.stat"))?;
        Ok(ResourceCounters {
            memory_limit_events: counter(&memory, "max")
                .saturating_add(counter(&memory, "oom"))
                .saturating_add(counter(&memory, "oom_kill")),
            pids_limit_events: counter(&pids, "max"),
            cpu_throttled_events: counter(&cpu, "nr_throttled"),
        })
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        write_required(&self.path.join("cgroup.kill"), "1")
    }

    pub(crate) fn wait_empty(&self, timeout: Duration) -> io::Result<()> {
        let started = Instant::now();
        loop {
            if self.process_ids()?.is_empty() {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "cgroup remained populated",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn cleanup(&mut self) -> io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        if self.path.exists() {
            // A failed partial setup can leave a regular test directory (or a
            // torn-down cgroup) without the control file. It is no longer a
            // live cgroup, so remove only this already-owned child and mark the
            // retry record clean after that removal succeeds.
            let pids = match self.process_ids() {
                Ok(pids) => pids,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::remove_dir_all(&self.path)?;
                    self.cleaned = true;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            if !pids.is_empty() {
                self.kill()?;
                self.wait_empty(Duration::from_secs(2))?;
            }
            fs::remove_dir(&self.path)?;
        }
        self.cleaned = true;
        Ok(())
    }

    pub(crate) fn opaque_id(&self) -> Option<&str> {
        self.path.file_name().and_then(|name| name.to_str())
    }
    pub(crate) fn private_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CgroupOperation {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn discover_current_delegated_cgroup() -> io::Result<PathBuf> {
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "not in unified cgroup v2"))?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    let root = Path::new("/sys/fs/cgroup").join(relative);
    let canonical = fs::canonicalize(root)?;
    if !canonical.starts_with("/sys/fs/cgroup") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "delegated cgroup escaped mount",
        ));
    }
    Ok(canonical)
}

pub(crate) fn verify_delegation(parent: &Path) -> io::Result<()> {
    let controllers = fs::read_to_string(parent.join("cgroup.controllers"))?;
    let enabled = fs::read_to_string(parent.join("cgroup.subtree_control"))?;
    for controller in ["memory", "pids", "cpu"] {
        if !controllers
            .split_whitespace()
            .any(|value| value == controller)
            || !enabled
                .split_whitespace()
                .any(|value| value.trim_start_matches('+') == controller)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("required cgroup controller unavailable: {controller}"),
            ));
        }
    }
    if fs::OpenOptions::new()
        .write(true)
        .open(parent.join("cgroup.procs"))
        .is_err()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cgroup delegation is not writable",
        ));
    }
    Ok(())
}

fn read_named_counters(path: &Path) -> io::Result<Vec<(String, u64)>> {
    fs::read_to_string(path)?
        .lines()
        .map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing counter name")
            })?;
            let value = parts
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing counter value"))?
                .parse::<u64>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid counter value"))?;
            if parts.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected counter fields",
                ));
            }
            Ok((name.to_string(), value))
        })
        .collect()
}

fn counter(values: &[(String, u64)], name: &str) -> u64 {
    values
        .iter()
        .find_map(|(found, value)| (found == name).then_some(*value))
        .unwrap_or(0)
}

fn write_required(path: &Path, value: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(value.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_parent() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pastey-cgroup-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("cgroup.controllers"), "memory pids cpu").unwrap();
        fs::write(root.join("cgroup.subtree_control"), "+memory +pids +cpu").unwrap();
        fs::write(root.join("cgroup.procs"), "").unwrap();
        root
    }

    fn create_fake_operation(parent: &Path) -> CgroupOperation {
        verify_delegation(parent).unwrap();
        let path = parent.join(format!("pastey-transform-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        for file in [
            "memory.max",
            "pids.max",
            "cpu.max",
            "cgroup.procs",
            "cgroup.kill",
        ] {
            fs::write(path.join(file), "").unwrap();
        }
        let mut operation = CgroupOperation {
            parent: parent.into(),
            path,
            cleaned: false,
        };
        operation.configure(TEXT_TRANSFORM_RESOURCE_POLICY).unwrap();
        operation
    }

    #[test]
    fn configures_attaches_kills_and_cleans_idempotently() {
        let parent = fake_parent();
        let mut operation = create_fake_operation(&parent);
        operation.attach(42).unwrap();
        assert_eq!(operation.process_ids().unwrap(), vec![42]);
        operation.kill().unwrap();
        operation.kill().unwrap();
        fs::write(operation.path.join("cgroup.procs"), "").unwrap();
        for file in [
            "memory.max",
            "pids.max",
            "cpu.max",
            "cgroup.procs",
            "cgroup.kill",
        ] {
            fs::remove_file(operation.path.join(file)).unwrap();
        }
        operation.cleanup().unwrap();
        operation.cleanup().unwrap();
        assert!(!operation.path.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn unavailable_controller_and_non_writable_shape_fail_closed() {
        let parent = fake_parent();
        fs::write(parent.join("cgroup.controllers"), "memory cpu").unwrap();
        assert!(verify_delegation(&parent).is_err());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn partial_configuration_is_cleanup_safe() {
        let parent = fake_parent();
        let path = parent.join(format!("pastey-transform-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("cgroup.procs"), "").unwrap();
        fs::write(path.join("cgroup.kill"), "").unwrap();
        let mut operation = CgroupOperation {
            parent: parent.clone(),
            path: path.clone(),
            cleaned: false,
        };
        assert!(operation.configure(TEXT_TRANSFORM_RESOURCE_POLICY).is_err());
        fs::remove_file(path.join("cgroup.procs")).unwrap();
        fs::remove_file(path.join("cgroup.kill")).unwrap();
        operation.cleanup().unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn resource_counter_parser_is_exact_and_bounded() {
        let parent = fake_parent();
        let path = parent.join("events");
        fs::write(&path, "max 2\noom 1\noom_kill 1\n").unwrap();
        let values = read_named_counters(&path).unwrap();
        assert_eq!(counter(&values, "max"), 2);
        assert_eq!(counter(&values, "oom_kill"), 1);
        assert_eq!(counter(&values, "missing"), 0);
        fs::write(&path, "invalid\n").unwrap();
        assert!(read_named_counters(&path).is_err());
        fs::remove_dir_all(parent).unwrap();
    }
}
