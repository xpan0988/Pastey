use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;

const LOG_FILE_NAME: &str = "pastey.log";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const ROTATED_LOGS_TO_KEEP: usize = 2;

#[derive(Default)]
struct LogState {
    logs_dir: Option<PathBuf>,
    last_error: Option<String>,
}

static LOG_STATE: OnceLock<Mutex<LogState>> = OnceLock::new();

pub fn init(logs_dir: PathBuf) {
    let mut state = log_state().lock();
    if fs::create_dir_all(&logs_dir).is_ok() {
        state.logs_dir = Some(logs_dir);
    }
}

pub fn write_transfer_line(line: &str) {
    write_line(line, false);
}

pub fn write_error_line(line: &str) {
    write_line(line, true);
}

pub fn latest_error_summary(logs_dir: &Path) -> Option<String> {
    if let Some(last_error) = log_state().lock().last_error.clone() {
        return Some(last_error);
    }

    latest_error_from_files(logs_dir)
}

pub fn log_file_path(logs_dir: &Path) -> PathBuf {
    logs_dir.join(LOG_FILE_NAME)
}

fn write_line(line: &str, is_error: bool) {
    let mut state = log_state().lock();
    if is_error {
        state.last_error = Some(summarize_error_line(line));
    }

    let Some(logs_dir) = state.logs_dir.clone() else {
        return;
    };

    let _ = fs::create_dir_all(&logs_dir);
    let log_path = log_file_path(&logs_dir);
    let _ = rotate_logs_if_needed(&logs_dir, &log_path);

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "time={} {}", now_unix_seconds(), line);
    }
}

fn rotate_logs_if_needed(logs_dir: &Path, log_path: &Path) -> std::io::Result<()> {
    let current_size = match fs::metadata(log_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if current_size < MAX_LOG_BYTES {
        return Ok(());
    }

    let oldest = rotated_log_path(logs_dir, ROTATED_LOGS_TO_KEEP);
    let _ = fs::remove_file(oldest);

    for index in (1..ROTATED_LOGS_TO_KEEP).rev() {
        let from = rotated_log_path(logs_dir, index);
        let to = rotated_log_path(logs_dir, index + 1);
        if from.exists() {
            let _ = fs::rename(from, to);
        }
    }

    fs::rename(log_path, rotated_log_path(logs_dir, 1))
}

fn latest_error_from_files(logs_dir: &Path) -> Option<String> {
    for path in log_search_paths(logs_dir) {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines().rev() {
                if is_error_line(line) {
                    return Some(summarize_error_line(line));
                }
            }
        }
    }
    None
}

fn log_search_paths(logs_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![log_file_path(logs_dir)];
    for index in 1..=ROTATED_LOGS_TO_KEEP {
        paths.push(rotated_log_path(logs_dir, index));
    }
    paths
}

fn rotated_log_path(logs_dir: &Path, index: usize) -> PathBuf {
    logs_dir.join(format!("pastey.{index}.log"))
}

fn is_error_line(line: &str) -> bool {
    line.contains("event=final_error")
        || line.contains("event=chunk_failure")
        || line.contains("event=start_failure")
        || line.contains("event=finalize_failure")
}

fn summarize_error_line(line: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 2_000;
    let mut summary = line.trim().to_string();
    if summary.len() > MAX_SUMMARY_CHARS {
        summary.truncate(MAX_SUMMARY_CHARS);
        summary.push_str("...");
    }
    summary
}

fn log_state() -> &'static Mutex<LogState> {
    LOG_STATE.get_or_init(|| Mutex::new(LogState::default()))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_error_finds_recent_transfer_failure() {
        let dir = std::env::temp_dir().join(format!("pastey_log_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            log_file_path(&dir),
            "time=1 event=chunk_ack\n\
             time=2 [pastey transfer][sender][transfer_id=t][room_id=r] event=final_error error_kind=timeout message=\"Transfer timed out.\"\n",
        )
        .unwrap();

        let latest = latest_error_from_files(&dir).unwrap();

        assert!(latest.contains("event=final_error"));
        assert!(latest.contains("Transfer timed out."));
        let _ = fs::remove_dir_all(dir);
    }
}
