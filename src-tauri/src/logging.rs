use std::{
    collections::HashMap,
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
const MAX_CHANGED_TRANSFER_LINES: usize = 4_096;

#[derive(Default)]
struct LogState {
    logs_dir: Option<PathBuf>,
    last_error: Option<String>,
    changed_transfer_lines: HashMap<String, String>,
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

/// Writes a diagnostic only when its authoritative projection changes. This is
/// intended for read-path diagnostics that may be recomputed by renderer
/// polling; it preserves real state changes without turning reads into events.
pub fn write_transfer_line_if_changed(key: &str, line: &str) {
    let should_write = {
        let mut state = log_state().lock();
        record_changed_transfer_line(&mut state, key, line)
    };
    if !should_write {
        return;
    }

    #[cfg(debug_assertions)]
    eprintln!("{line}");
    write_line(line, false);
}

fn record_changed_transfer_line(state: &mut LogState, key: &str, line: &str) -> bool {
    if state
        .changed_transfer_lines
        .get(key)
        .is_some_and(|last| last == line)
    {
        return false;
    }
    if state.changed_transfer_lines.len() >= MAX_CHANGED_TRANSFER_LINES {
        state.changed_transfer_lines.clear();
    }
    state
        .changed_transfer_lines
        .insert(key.to_string(), line.to_string());
    true
}

pub fn write_error_line(line: &str) {
    write_line(line, true);
}

const RUN_MARKER_FILE_NAME: &str = "pastey.running";
const MAX_PANIC_MESSAGE_CHARS: usize = 300;

static CRASH_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Leaves evidence of a crash in the log. Release builds abort on panic and
/// strip symbols, so without this a panic on any thread ends the process
/// with nothing recorded. The run marker also reveals exits that run no
/// panic hook (an access violation or stack overflow): it survives only an
/// unclean exit and is reported on the next start.
pub fn install_crash_reporting(app_version: &str) {
    let Some(logs_dir) = log_state().lock().logs_dir.clone() else {
        return;
    };
    let _ = CRASH_LOG_PATH.set(log_file_path(&logs_dir));
    let marker = logs_dir.join(RUN_MARKER_FILE_NAME);
    if let Ok(previous) = fs::read_to_string(&marker) {
        write_error_line(&format!(
            "[pastey lifecycle] event=previous_run_ended_uncleanly {}",
            previous.trim().replace(['\n', '\r'], " ")
        ));
    }
    let _ = fs::write(
        &marker,
        format!(
            "started_at={} pid={} version={}",
            now_unix_seconds(),
            std::process::id(),
            app_version
        ),
    );
    write_transfer_line(&format!(
        "[pastey lifecycle] event=started pid={} version={}",
        std::process::id(),
        app_version
    ));

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".into());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-text panic payload".into());
        let line = panic_log_line(thread.name().unwrap_or("unnamed"), &location, &message);
        // Written without the log mutex: the panicking thread may hold it.
        if let Some(path) = CRASH_LOG_PATH.get() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "time={} {}", now_unix_seconds(), line);
                let _ = file.sync_all();
            }
        }
        previous_hook(info);
    }));
}

/// Removes the run marker on an orderly exit.
pub fn record_clean_exit() {
    if let Some(logs_dir) = log_state().lock().logs_dir.clone() {
        let _ = fs::remove_file(logs_dir.join(RUN_MARKER_FILE_NAME));
    }
    write_transfer_line("[pastey lifecycle] event=exited");
}

/// One bounded, single-line record of a panic.
pub(crate) fn panic_log_line(thread: &str, location: &str, message: &str) -> String {
    let mut message = message.replace(['\n', '\r'], " ");
    if message.chars().count() > MAX_PANIC_MESSAGE_CHARS {
        message = message
            .chars()
            .take(MAX_PANIC_MESSAGE_CHARS)
            .collect::<String>()
            + "…";
    }
    format!("[pastey crash] event=panic thread={thread} location={location} message={message}")
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
    fn a_panic_is_recorded_on_one_bounded_line() {
        let line = panic_log_line(
            "tokio-runtime-worker",
            "src/native_agent.rs:42",
            "boom\nsecond line",
        );
        assert_eq!(
            line,
            "[pastey crash] event=panic thread=tokio-runtime-worker location=src/native_agent.rs:42 message=boom second line"
        );
        let long = "界".repeat(MAX_PANIC_MESSAGE_CHARS + 50);
        let bounded = panic_log_line("main", "x.rs:1", &long);
        assert!(bounded.ends_with('…'));
        assert_eq!(bounded.matches('界').count(), MAX_PANIC_MESSAGE_CHARS);
        assert!(!bounded.contains('\n'));
    }

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

    #[test]
    fn changed_transfer_diagnostics_are_idempotent_until_projection_changes() {
        let mut state = LogState::default();
        let key = "room_item_render_kind:item-a";
        let first = "event=room_item_render_kind status=sent";
        let changed = "event=room_item_render_kind status=failed";

        assert!(record_changed_transfer_line(&mut state, key, first));
        assert!(!record_changed_transfer_line(&mut state, key, first));
        assert!(record_changed_transfer_line(&mut state, key, changed));
    }
}
