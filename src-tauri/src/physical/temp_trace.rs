//! TEMP-TRACE: temporary lifecycle tracing for the remote stream close
//! investigation. Remove this file and every line marked TEMP-TRACE.
//!
//! Events are formatted into an in-memory queue (no IO) and printed to stderr
//! only by `flush`, which callers invoke after releasing the Core lock.
//! Normal ticks only touch atomics.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static START: OnceLock<Instant> = OnceLock::new();
static PENDING: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
/// Full ledger audits run so far, and their total duration.
pub(crate) static FULL_AUDITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static FULL_AUDIT_US: AtomicU64 = AtomicU64::new(0);
/// Ledger reconnects after an outside change to the file (stamp mismatch).
pub(crate) static RECONNECTS: AtomicU64 = AtomicU64::new(0);
/// Legacy bridge-peer projection syncs reached, and how many wrote.
pub(crate) static SYNC_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SYNC_WRITES: AtomicU64 = AtomicU64::new(0);
/// Commits through the physical ledger connection.
pub(crate) static PHYSICAL_COMMITS: AtomicU64 = AtomicU64::new(0);
static OBSERVER: parking_lot::Mutex<Option<rusqlite::Connection>> = parking_lot::Mutex::new(None);
/// `PRAGMA data_version` on a connection that never writes: it changes when
/// any other connection (Pastey's own included) has committed to the file.
/// Never call it with the Core lock held.
pub(crate) fn data_version(path: &std::path::Path) -> i64 {
    let mut observer = OBSERVER.lock();
    if observer.is_none() {
        *observer = rusqlite::Connection::open(path).ok();
    }
    observer
        .as_ref()
        .and_then(|c| c.query_row("PRAGMA data_version", [], |r| r.get(0)).ok())
        .unwrap_or(-1)
}
/// Counters at the start of a traced window.
pub(crate) struct WindowV1 {
    data_version: i64,
    sync_calls: u64,
    sync_writes: u64,
    physical_commits: u64,
    full_audits: u64,
    reconnects: u64,
}
impl WindowV1 {
    pub(crate) fn open(path: &std::path::Path) -> Self {
        Self {
            data_version: data_version(path),
            sync_calls: SYNC_CALLS.load(Ordering::Relaxed),
            sync_writes: SYNC_WRITES.load(Ordering::Relaxed),
            physical_commits: PHYSICAL_COMMITS.load(Ordering::Relaxed),
            full_audits: FULL_AUDITS.load(Ordering::Relaxed),
            reconnects: RECONNECTS.load(Ordering::Relaxed),
        }
    }
    /// What happened since `open`. Never call it with the Core lock held.
    pub(crate) fn close(&self, path: &std::path::Path) -> String {
        format!(
            "db_changed={} sync_calls+={} sync_writes+={} physical_commits+={} full_audits+={} reconnects+={} (totals sync_calls={} sync_writes={})",
            data_version(path) != self.data_version,
            SYNC_CALLS.load(Ordering::Relaxed) - self.sync_calls,
            SYNC_WRITES.load(Ordering::Relaxed) - self.sync_writes,
            PHYSICAL_COMMITS.load(Ordering::Relaxed) - self.physical_commits,
            FULL_AUDITS.load(Ordering::Relaxed) - self.full_audits,
            RECONNECTS.load(Ordering::Relaxed) - self.reconnects,
            SYNC_CALLS.load(Ordering::Relaxed),
            SYNC_WRITES.load(Ordering::Relaxed),
        )
    }
}
/// Room Control events received by kind, and rejections answered by code.
static RC_EVENTS: parking_lot::Mutex<std::collections::BTreeMap<String, u64>> = parking_lot::Mutex::new(std::collections::BTreeMap::new());
/// Exact-peer liveness probes (`resolve_current_remote_host_session`) and
/// capability queries sent by this Host.
pub(crate) static PROBES: AtomicU64 = AtomicU64::new(0);
pub(crate) static CAPABILITY_QUERIES: AtomicU64 = AtomicU64::new(0);
pub(crate) fn count_event(key: String) {
    *RC_EVENTS.lock().entry(key).or_default() += 1;
}
/// Room Control totals for this Host.
pub(crate) fn room_control() -> String {
    format!(
        "rc_events={:?} probes={} capability_queries_sent={}",
        RC_EVENTS.lock().clone(),
        PROBES.load(Ordering::Relaxed),
        CAPABILITY_QUERIES.load(Ordering::Relaxed)
    )
}
/// Per-decision timing: the latest executor-side tool_call receipt, the
/// latest admission and prepare-write ledger durations, supervisor ticks.
pub(crate) static LAST_TOOL_CALL_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST_ADMIT_WRITE_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST_PREPARE_WRITE_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static SUPERVISOR_TICKS: AtomicU64 = AtomicU64::new(0);
/// Root validations, each of which runs the route check on a remote stream.
pub(crate) static VALIDATE_ROOTS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SUPERVISOR_TICK_US: AtomicU64 = AtomicU64::new(0);
/// Physical route checks run so far, and their total duration.
pub(crate) static ROUTE_CHECKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static ROUTE_CHECK_US: AtomicU64 = AtomicU64::new(0);

/// Microseconds on a monotonic clock since first use.
pub(crate) fn mono_us() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}
/// Monotonic microseconds and wall-clock milliseconds.
pub(crate) fn stamp() -> String {
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("mono_us={} wall_ms={wall}", mono_us())
}
/// The last eight characters of an identifier.
pub(crate) fn short<T: Clone>(id: &T) -> String
where
    String: From<T>,
{
    let id = String::from(id.clone());
    id[id.len().saturating_sub(8)..].to_owned()
}
/// Queues one event; never does IO.
pub(crate) fn push(event: String) {
    PENDING.lock().push(event);
}
/// Prints queued events. Call only without the Core lock held.
pub(crate) fn flush() {
    let events = std::mem::take(&mut *PENDING.lock());
    for e in events {
        eprintln!("TEMP-TRACE {e}");
    }
}
/// Ledger audit counters as one string.
pub(crate) fn audits() -> String {
    format!(
        "full_audits={} full_audit_us={} reconnects={} route_checks={} route_check_us={}",
        FULL_AUDITS.load(Ordering::Relaxed),
        FULL_AUDIT_US.load(Ordering::Relaxed),
        RECONNECTS.load(Ordering::Relaxed),
        ROUTE_CHECKS.load(Ordering::Relaxed),
        ROUTE_CHECK_US.load(Ordering::Relaxed)
    )
}

/// Per-stream marks. Atomics only on the normal path; the end reason and its
/// detail are written once, on the way out.
#[derive(Default)]
pub(crate) struct StreamTraceV1 {
    pub last_capture: AtomicU64,
    pub max_gap_us: AtomicU64,
    pub max_tick_us: AtomicU64,
    pub samples: AtomicU64,
    /// Set by the supervisor at each tick start.
    pub tick_started_us: AtomicU64,
    pub audits_at_tick: AtomicU64,
    pub audit_us_at_tick: AtomicU64,
    pub audits_at_start: AtomicU64,
    pub audit_us_at_start: AtomicU64,
    end: parking_lot::Mutex<Option<(&'static str, String)>>,
    /// Latest and largest duration of each tick phase, in `PHASES` order.
    pub phase_us: [AtomicU64; 4],
    pub phase_max_us: [AtomicU64; 4],
}
/// The phases of a stream tick.
pub(crate) const PHASES: [&str; 4] = ["validate", "sample", "effect_bound", "evaluate_budget"];
impl StreamTraceV1 {
    /// Marks the start of a supervisor tick (atomics only).
    pub(crate) fn tick(&self) {
        self.tick_started_us.store(mono_us(), Ordering::Relaxed);
        self.audits_at_tick
            .store(FULL_AUDITS.load(Ordering::Relaxed), Ordering::Relaxed);
        self.audit_us_at_tick
            .store(FULL_AUDIT_US.load(Ordering::Relaxed), Ordering::Relaxed);
        if self.audits_at_start.load(Ordering::Relaxed) == 0 {
            self.audits_at_start.store(
                FULL_AUDITS.load(Ordering::Relaxed) + 1,
                Ordering::Relaxed,
            );
            self.audit_us_at_start
                .store(FULL_AUDIT_US.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }
    /// Records one phase's duration (atomics only).
    pub(crate) fn phase(&self, i: usize, started: Instant) {
        let us = started.elapsed().as_micros() as u64;
        self.phase_us[i].store(us, Ordering::Relaxed);
        self.phase_max_us[i].fetch_max(us, Ordering::Relaxed);
    }
    fn phases(&self, of: &[AtomicU64; 4]) -> String {
        (0..4)
            .map(|i| format!("{}={}", PHASES[i], of[i].load(Ordering::Relaxed)))
            .collect::<Vec<_>>()
            .join(" ")
    }
    /// Time and full audits since the current tick started.
    pub(crate) fn this_tick(&self) -> String {
        let started = self.tick_started_us.load(Ordering::Relaxed);
        format!(
            "phases_us({}) tick_elapsed_us={} full_audits_this_tick={} full_audit_us_this_tick={}",
            self.phases(&self.phase_us),
            if started == 0 { 0 } else { mono_us().saturating_sub(started) },
            FULL_AUDITS
                .load(Ordering::Relaxed)
                .saturating_sub(self.audits_at_tick.load(Ordering::Relaxed)),
            FULL_AUDIT_US
                .load(Ordering::Relaxed)
                .saturating_sub(self.audit_us_at_tick.load(Ordering::Relaxed))
        )
    }
    /// Records why `end_stream` is about to be called; the first reason wins.
    pub(crate) fn note_end(&self, why: &'static str, detail: String) {
        self.end.lock().get_or_insert((why, detail));
    }
    pub(crate) fn end_reason(&self) -> (&'static str, String) {
        self.end
            .lock()
            .clone()
            .unwrap_or(("unnoted", String::new()))
    }
    pub(crate) fn summary(&self) -> String {
        let since = self.audits_at_start.load(Ordering::Relaxed);
        format!(
            "max_phase_us({}) samples={} max_gap_us={} max_tick_us={} full_audits_during_stream={} full_audit_us_during_stream={}",
            self.phases(&self.phase_max_us),
            self.samples.load(Ordering::Relaxed),
            self.max_gap_us.load(Ordering::Relaxed),
            self.max_tick_us.load(Ordering::Relaxed),
            if since == 0 { 0 } else { FULL_AUDITS.load(Ordering::Relaxed).saturating_sub(since - 1) },
            FULL_AUDIT_US
                .load(Ordering::Relaxed)
                .saturating_sub(self.audit_us_at_start.load(Ordering::Relaxed)),
        )
    }
}
