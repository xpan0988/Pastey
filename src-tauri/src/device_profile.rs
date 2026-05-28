use std::process::Command;

use crate::{
    config::StoredConfig,
    diagnostics::{DeviceProfile, PowerState},
    storage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileProbeMode {
    Quick,
    Full,
}

pub fn local_device_profile_with_mode(
    config: &StoredConfig,
    mode: ProfileProbeMode,
) -> DeviceProfile {
    let cpu = cpu_info(mode);
    DeviceProfile {
        device_id: config.device_id.clone(),
        device_name: device_name(),
        platform: std::env::consts::OS.to_string(),
        os_version: os_version(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_name: cpu.name,
        cpu_physical_core_count: cpu.physical_core_count,
        cpu_logical_processor_count: cpu.logical_processor_count,
        cpu_core_count: cpu.logical_processor_count.or(cpu.physical_core_count),
        memory_total_gb: memory_total_gb(mode),
        gpu_names: gpu_names(mode),
        power_state: power_state(),
        battery_percent: battery_percent(),
        updated_at: storage::now_ts(),
    }
}

#[derive(Clone, Debug, Default)]
struct CpuInfo {
    name: Option<String>,
    physical_core_count: Option<usize>,
    logical_processor_count: Option<usize>,
}

fn device_name() -> String {
    #[cfg(target_os = "macos")]
    {
        first_non_empty([
            fixed_command_text("/usr/sbin/scutil", &["--get", "ComputerName"]),
            fixed_command_text("/usr/sbin/scutil", &["--get", "LocalHostName"]),
            fixed_command_text("/usr/sbin/scutil", &["--get", "HostName"]),
            fixed_command_text("/bin/hostname", &[]),
            std::env::var("HOSTNAME").ok(),
        ])
        .unwrap_or_else(|| "Mac".into())
    }

    #[cfg(target_os = "windows")]
    {
        first_non_empty([
            windows_computer_name(5),
            windows_computer_name(1),
            windows_computer_name(0),
            std::env::var("COMPUTERNAME").ok(),
        ])
        .unwrap_or_else(|| "Windows PC".into())
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        first_non_empty([
            fixed_command_text("/bin/hostname", &[]),
            fixed_command_text("hostname", &[]),
            std::env::var("HOSTNAME").ok(),
        ])
        .unwrap_or_else(|| "Nearby device".into())
    }
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        fixed_command_text("/usr/bin/sw_vers", &["-productVersion"])
            .or_else(|| fixed_command_text("sw_vers", &["-productVersion"]))
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var("OS")
            .ok()
            .filter(|value| !value.trim().is_empty())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("PRETTY_NAME=")
                        .map(|value| value.trim_matches('"').to_string())
                })
            })
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

fn cpu_info(mode: ProfileProbeMode) -> CpuInfo {
    #[cfg(target_os = "macos")]
    {
        let hardware = if mode == ProfileProbeMode::Full {
            fixed_command_text("/usr/sbin/system_profiler", &["SPHardwareDataType"])
                .or_else(|| fixed_command_text("system_profiler", &["SPHardwareDataType"]))
        } else {
            None
        };
        let chip_name = hardware
            .as_deref()
            .and_then(parse_macos_chip_name)
            .or_else(|| {
                fixed_command_text("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
                    .or_else(|| fixed_command_text("sysctl", &["-n", "machdep.cpu.brand_string"]))
            });
        let logical = hardware
            .as_deref()
            .and_then(parse_macos_total_cores)
            .or_else(|| sysctl_usize("hw.ncpu"))
            .or_else(logical_parallelism);
        let name = if let Some(chip) = chip_name {
            Some(chip)
        } else if std::env::consts::ARCH == "aarch64" {
            Some("Apple Silicon".into())
        } else {
            fixed_command_text("/usr/sbin/sysctl", &["-n", "hw.model"])
                .or_else(|| fixed_command_text("sysctl", &["-n", "hw.model"]))
        };

        CpuInfo {
            name,
            physical_core_count: logical,
            logical_processor_count: logical,
        }
    }

    #[cfg(target_os = "windows")]
    {
        windows_cpu_info(mode)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let name = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("model name")
                        .and_then(|rest| {
                            rest.split_once(':')
                                .map(|(_, value)| value.trim().to_string())
                        })
                        .filter(|value| !value.is_empty())
                })
            });
        let logical = logical_parallelism();
        CpuInfo {
            name,
            physical_core_count: logical,
            logical_processor_count: logical,
        }
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        CpuInfo {
            logical_processor_count: logical_parallelism(),
            ..CpuInfo::default()
        }
    }
}

fn memory_total_gb(_mode: ProfileProbeMode) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let bytes = fixed_command_text("/usr/sbin/sysctl", &["-n", "hw.memsize"])
            .or_else(|| fixed_command_text("sysctl", &["-n", "hw.memsize"]))?
            .parse::<u64>()
            .ok()?;
        Some(bytes_to_gb(bytes))
    }

    #[cfg(target_os = "windows")]
    {
        if _mode == ProfileProbeMode::Quick {
            return None;
        }

        fixed_command_text(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ],
        )
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(bytes_to_gb)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let kb = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("MemTotal:")
                        .and_then(|rest| rest.split_whitespace().next())
                        .and_then(|value| value.parse::<u64>().ok())
                })
            })?;
        Some(bytes_to_gb(kb * 1024))
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

fn gpu_names(mode: ProfileProbeMode) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        if mode == ProfileProbeMode::Quick {
            return if std::env::consts::ARCH == "aarch64" {
                vec!["Apple GPU".into()]
            } else {
                Vec::new()
            };
        }

        let names = fixed_command_text("/usr/sbin/system_profiler", &["SPDisplaysDataType"])
            .or_else(|| fixed_command_text("system_profiler", &["SPDisplaysDataType"]))
            .map(|output| parse_macos_gpu_names(&output))
            .unwrap_or_default();
        if names.is_empty() && std::env::consts::ARCH == "aarch64" {
            vec!["Apple GPU".into()]
        } else {
            names
        }
    }

    #[cfg(target_os = "windows")]
    {
        if mode == ProfileProbeMode::Quick {
            return Vec::new();
        }

        fixed_command_text(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }",
            ],
        )
        .map(|output| parse_lines(&output))
        .unwrap_or_default()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

fn power_state() -> PowerState {
    #[cfg(target_os = "macos")]
    {
        let Some(output) = fixed_command_text("/usr/bin/pmset", &["-g", "batt"])
            .or_else(|| fixed_command_text("pmset", &["-g", "batt"]))
        else {
            return PowerState::Unknown;
        };
        if output.contains("AC Power") {
            PowerState::PluggedIn
        } else if output.contains("Battery Power") {
            PowerState::OnBattery
        } else {
            PowerState::Unknown
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        PowerState::Unknown
    }
}

fn battery_percent() -> Option<u8> {
    #[cfg(target_os = "macos")]
    {
        let output = fixed_command_text("/usr/bin/pmset", &["-g", "batt"])
            .or_else(|| fixed_command_text("pmset", &["-g", "batt"]))?;
        parse_battery_percent(&output)
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn fixed_command_text(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn first_non_empty(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && value != "(null)")
}

fn logical_parallelism() -> Option<usize> {
    std::thread::available_parallelism()
        .ok()
        .map(|count| count.get())
}

#[cfg(target_os = "macos")]
fn sysctl_usize(name: &str) -> Option<usize> {
    fixed_command_text("/usr/sbin/sysctl", &["-n", name])
        .or_else(|| fixed_command_text("sysctl", &["-n", name]))?
        .trim()
        .parse::<usize>()
        .ok()
}

fn bytes_to_gb(bytes: u64) -> u64 {
    let gb = 1024_u64.pow(3);
    (bytes + gb / 2) / gb
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_gpu_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("Chipset Model:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_chip_name(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Chip:")
            .or_else(|| line.trim().strip_prefix("Processor Name:"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_total_cores(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Total Number of Cores:")
            .or_else(|| line.trim().strip_prefix("Number of Processors:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<usize>().ok())
    })
}

#[cfg(target_os = "windows")]
fn parse_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_battery_percent(output: &str) -> Option<u8> {
    let (_, rest) = output.split_once('%')?;
    let before_percent = output[..output.len() - rest.len() - 1]
        .rsplit_once(char::is_whitespace)
        .map(|(_, value)| value)
        .unwrap_or(output);
    before_percent.trim().parse::<u8>().ok()
}

#[cfg(target_os = "windows")]
fn windows_cpu_info(mode: ProfileProbeMode) -> CpuInfo {
    if mode == ProfileProbeMode::Quick {
        return CpuInfo {
            name: std::env::var("PROCESSOR_IDENTIFIER")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            logical_processor_count: logical_parallelism(),
            ..CpuInfo::default()
        };
    }

    let output = fixed_command_text(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Processor | Select-Object -First 1 Name,NumberOfCores,NumberOfLogicalProcessors | ConvertTo-Json -Compress",
        ],
    );
    output
        .as_deref()
        .and_then(parse_windows_cpu_info)
        .unwrap_or_else(|| CpuInfo {
            name: std::env::var("PROCESSOR_IDENTIFIER")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            logical_processor_count: logical_parallelism(),
            ..CpuInfo::default()
        })
}

#[cfg(any(target_os = "windows", test))]
fn parse_windows_cpu_info(output: &str) -> Option<CpuInfo> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    Some(CpuInfo {
        name: value
            .get("Name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        physical_core_count: value
            .get("NumberOfCores")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()),
        logical_processor_count: value
            .get("NumberOfLogicalProcessors")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()),
    })
}

#[cfg(target_os = "windows")]
fn windows_computer_name(format: u32) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetComputerNameExW(format: u32, buffer: *mut u16, size: *mut u32) -> i32;
    }

    let mut size = 0u32;
    unsafe {
        let _ = GetComputerNameExW(format, std::ptr::null_mut(), &mut size);
    }
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    let ok = unsafe { GetComputerNameExW(format, buffer.as_mut_ptr(), &mut size) };
    if ok == 0 || size == 0 {
        return None;
    }
    Some(
        OsString::from_wide(&buffer[..size as usize])
            .to_string_lossy()
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_gpu_parser_reads_chipset_names() {
        let names = parse_macos_gpu_names(
            r#"
Graphics/Displays:

    Apple M5:

      Chipset Model: Apple M5
      Type: GPU
            "#,
        );

        assert_eq!(names, vec!["Apple M5"]);
    }

    #[test]
    fn macos_chip_parser_reads_user_friendly_chip_name() {
        let chip = parse_macos_chip_name(
            r#"
Hardware:

    Hardware Overview:

      Chip: Apple M5
      Total Number of Cores: 10
            "#,
        );

        assert_eq!(chip, Some("Apple M5".into()));
    }

    #[test]
    fn macos_core_parser_reads_total_core_count() {
        assert_eq!(
            parse_macos_total_cores("Total Number of Cores: 10 (4 performance and 6 efficiency)"),
            Some(10)
        );
    }

    #[test]
    fn windows_cpu_parser_reads_name_cores_and_threads() {
        let cpu = parse_windows_cpu_info(
            r#"{"Name":"AMD Ryzen 7 9800X3D 8-Core Processor","NumberOfCores":8,"NumberOfLogicalProcessors":16}"#,
        )
        .unwrap();

        assert_eq!(
            cpu.name,
            Some("AMD Ryzen 7 9800X3D 8-Core Processor".into())
        );
        assert_eq!(cpu.physical_core_count, Some(8));
        assert_eq!(cpu.logical_processor_count, Some(16));
    }

    #[test]
    fn device_name_fallback_uses_first_real_name() {
        let name = first_non_empty([
            None,
            Some("   ".into()),
            Some("(null)".into()),
            Some("Xiyuans-MacBook-Air".into()),
        ]);

        assert_eq!(name, Some("Xiyuans-MacBook-Air".into()));
    }

    #[test]
    fn battery_parser_returns_percent_when_present() {
        assert_eq!(
            parse_battery_percent("Now drawing from 'AC Power'\n -InternalBattery-0 88%; charged"),
            Some(88)
        );
    }
}
