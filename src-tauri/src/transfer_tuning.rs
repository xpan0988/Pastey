use std::time::{Duration, Instant};

pub const DEFAULT_BINARY_V1_WINDOW: usize = 8;
pub const MIN_TRANSFER_WINDOW: usize = 1;
pub const MAX_TRANSFER_WINDOW: usize = 16;
const TRANSFER_WINDOW_ENV: &str = "PASTEY_TRANSFER_WINDOW_SIZE";

#[derive(Clone, Copy, Debug)]
pub struct TransferTuning {
    pub effective_window_size: usize,
    pub override_source: TransferWindowOverrideSource,
}

impl Default for TransferTuning {
    fn default() -> Self {
        Self {
            effective_window_size: DEFAULT_BINARY_V1_WINDOW,
            override_source: TransferWindowOverrideSource::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferWindowOverrideSource {
    Env,
    DevSettings,
    PlannerRequest,
    Default,
}

impl TransferWindowOverrideSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::DevSettings => "dev_settings",
            Self::PlannerRequest => "planner_request",
            Self::Default => "default",
        }
    }
}

pub fn normalize_transfer_window_override(value: Option<usize>) -> Option<usize> {
    value.map(clamp_transfer_window)
}

pub fn effective_transfer_tuning_from_env(
    dev_window_override: Option<usize>,
    dev_tools_enabled: bool,
    requested_window: Option<usize>,
) -> TransferTuning {
    let env_window_override = std::env::var(TRANSFER_WINDOW_ENV).ok();
    effective_transfer_tuning(
        dev_window_override,
        dev_tools_enabled,
        requested_window,
        env_window_override.as_deref(),
    )
}

pub fn effective_transfer_tuning(
    dev_window_override: Option<usize>,
    dev_tools_enabled: bool,
    requested_window: Option<usize>,
    env_window_override: Option<&str>,
) -> TransferTuning {
    if let Some(window_size) =
        env_window_override.and_then(|value| value.trim().parse::<usize>().ok())
    {
        return TransferTuning {
            effective_window_size: clamp_transfer_window(window_size),
            override_source: TransferWindowOverrideSource::Env,
        };
    }

    if dev_tools_enabled {
        if let Some(window_size) = dev_window_override {
            return TransferTuning {
                effective_window_size: clamp_transfer_window(window_size),
                override_source: TransferWindowOverrideSource::DevSettings,
            };
        }
    }

    if let Some(window_size) = requested_window {
        return TransferTuning {
            effective_window_size: clamp_transfer_window(window_size),
            override_source: TransferWindowOverrideSource::PlannerRequest,
        };
    }

    TransferTuning::default()
}

pub fn clamp_transfer_window(value: usize) -> usize {
    value.clamp(MIN_TRANSFER_WINDOW, MAX_TRANSFER_WINDOW)
}

/// Chunks in flight when a transfer starts on an internet tunnel.
pub const TUNNEL_INITIAL_WINDOW: usize = 2;
/// A chunk acknowledged faster than this lets one more chunk fly.
const TUNNEL_GROW_BELOW: Duration = Duration::from_secs(10);
/// A chunk slower than this halves the chunks in flight. Well inside the
/// chunk request deadline, so a slow link shrinks before chunks time out.
const TUNNEL_SHRINK_ABOVE: Duration = Duration::from_secs(30);

/// How many chunks one transfer keeps in flight on an internet tunnel.
///
/// On a LAN the configured window is right: the link is fast and every
/// chunk returns in milliseconds. Across the internet the same window puts
/// `window x chunk size` bytes on a link whose rate is unknown, and each
/// chunk must still return within its request deadline; on a slow link all
/// of them time out together and nothing is ever delivered. This starts
/// small and follows what the link shows: fast chunks add one, a slow chunk
/// halves. It only ever lowers the configured window (`ceiling`), never
/// raises it, and it is local to the sender: nothing on the wire changes.
#[derive(Clone, Copy, Debug)]
pub struct TunnelWindow {
    window: usize,
    /// Chunks issued before this were sized by an older window; their
    /// timings say nothing about the current one.
    changed_at: Instant,
}

impl TunnelWindow {
    pub fn new(now: Instant) -> Self {
        Self {
            window: TUNNEL_INITIAL_WINDOW,
            changed_at: now,
        }
    }

    /// Chunks that may be in flight now, given the configured window.
    pub fn limit(&self, ceiling: usize) -> usize {
        self.window.min(ceiling).max(MIN_TRANSFER_WINDOW)
    }

    /// Takes one acknowledged chunk's timing (issued at `issued_at`, it took
    /// `elapsed` including any retries). Returns the new limit if it changed.
    pub fn observe(
        &mut self,
        issued_at: Instant,
        elapsed: Duration,
        ceiling: usize,
        now: Instant,
    ) -> Option<usize> {
        let before = self.limit(ceiling);
        if elapsed > TUNNEL_SHRINK_ABOVE {
            // One slow window of chunks shrinks once, not once per chunk.
            if issued_at >= self.changed_at {
                self.window = (before / 2).max(MIN_TRANSFER_WINDOW);
                self.changed_at = now;
            }
        } else if elapsed < TUNNEL_GROW_BELOW && before < ceiling {
            self.window = before + 1;
        }
        let after = self.limit(ceiling);
        (after != before).then_some(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_window_normalization_clamps_supported_range() {
        assert_eq!(normalize_transfer_window_override(None), None);
        assert_eq!(normalize_transfer_window_override(Some(0)), Some(1));
        assert_eq!(normalize_transfer_window_override(Some(1)), Some(1));
        assert_eq!(normalize_transfer_window_override(Some(8)), Some(8));
        assert_eq!(normalize_transfer_window_override(Some(99)), Some(16));
    }

    #[test]
    fn default_binary_window_is_eight() {
        let tuning = effective_transfer_tuning(None, false, None, None);

        assert_eq!(tuning.effective_window_size, 8);
        assert_eq!(
            tuning.override_source,
            TransferWindowOverrideSource::Default
        );
    }

    #[test]
    fn env_window_override_is_clamped() {
        for (override_value, expected_window) in [
            ("1", 1),
            ("2", 2),
            ("4", 4),
            ("8", 8),
            ("16", 16),
            ("0", 1),
            ("99", 16),
        ] {
            let tuning = effective_transfer_tuning(None, false, Some(3), Some(override_value));
            assert_eq!(tuning.effective_window_size, expected_window);
            assert_eq!(tuning.override_source, TransferWindowOverrideSource::Env);
        }
    }

    #[test]
    fn env_window_override_takes_precedence_over_dev_setting() {
        let tuning = effective_transfer_tuning(Some(4), true, Some(2), Some("8"));

        assert_eq!(tuning.effective_window_size, 8);
        assert_eq!(tuning.override_source, TransferWindowOverrideSource::Env);
    }

    #[test]
    fn invalid_env_window_override_falls_back_to_dev_setting_or_default() {
        let dev_tuning = effective_transfer_tuning(Some(4), true, Some(2), Some("nope"));
        let planner_tuning = effective_transfer_tuning(Some(4), false, Some(2), Some("nope"));
        let default_tuning = effective_transfer_tuning(Some(4), false, None, Some("nope"));

        assert_eq!(dev_tuning.effective_window_size, 4);
        assert_eq!(
            dev_tuning.override_source,
            TransferWindowOverrideSource::DevSettings
        );
        assert_eq!(planner_tuning.effective_window_size, 2);
        assert_eq!(
            planner_tuning.override_source,
            TransferWindowOverrideSource::PlannerRequest
        );
        assert_eq!(default_tuning.effective_window_size, 8);
        assert_eq!(
            default_tuning.override_source,
            TransferWindowOverrideSource::Default
        );
    }

    #[test]
    fn dev_transfer_window_setting_maps_to_effective_window() {
        let tuning = effective_transfer_tuning(Some(16), true, Some(2), None);
        let hidden_tuning = effective_transfer_tuning(Some(16), false, None, None);

        assert_eq!(tuning.effective_window_size, 16);
        assert_eq!(
            tuning.override_source,
            TransferWindowOverrideSource::DevSettings
        );
        assert_eq!(hidden_tuning.effective_window_size, 8);
        assert_eq!(
            hidden_tuning.override_source,
            TransferWindowOverrideSource::Default
        );
    }

    #[test]
    fn requested_window_is_used_between_dev_setting_and_default() {
        let tuning = effective_transfer_tuning(None, false, Some(3), None);

        assert_eq!(tuning.effective_window_size, 3);
        assert_eq!(
            tuning.override_source,
            TransferWindowOverrideSource::PlannerRequest
        );
    }

    #[test]
    fn requested_window_is_clamped_to_supported_range() {
        let low = effective_transfer_tuning(None, false, Some(0), None);
        let high = effective_transfer_tuning(None, false, Some(99), None);

        assert_eq!(low.effective_window_size, 1);
        assert_eq!(
            low.override_source,
            TransferWindowOverrideSource::PlannerRequest
        );
        assert_eq!(high.effective_window_size, 16);
        assert_eq!(
            high.override_source,
            TransferWindowOverrideSource::PlannerRequest
        );
    }

    #[test]
    fn dev_setting_takes_precedence_over_requested_window() {
        let tuning = effective_transfer_tuning(Some(4), true, Some(2), None);

        assert_eq!(tuning.effective_window_size, 4);
        assert_eq!(
            tuning.override_source,
            TransferWindowOverrideSource::DevSettings
        );
    }

    #[test]
    fn tunnel_window_starts_small_and_grows_with_fast_chunks() {
        let start = Instant::now();
        let mut window = TunnelWindow::new(start);
        assert_eq!(window.limit(8), TUNNEL_INITIAL_WINDOW);
        let fast = Duration::from_secs(2);
        for expected in 3..=8 {
            assert_eq!(window.observe(start, fast, 8, start + fast), Some(expected));
        }
        // Never above the configured window.
        assert_eq!(window.observe(start, fast, 8, start + fast), None);
        assert_eq!(window.limit(8), 8);
    }

    #[test]
    fn tunnel_window_never_exceeds_a_smaller_configured_window() {
        let start = Instant::now();
        let mut window = TunnelWindow::new(start);
        assert_eq!(window.limit(1), 1);
        assert_eq!(
            window.observe(start, Duration::from_secs(1), 1, start),
            None
        );
        // The configured window was lowered while the transfer ran.
        let mut window = TunnelWindow::new(start);
        for _ in 0..6 {
            window.observe(start, Duration::from_secs(1), 8, start);
        }
        assert_eq!(window.limit(3), 3);
    }

    #[test]
    fn tunnel_window_holds_between_the_two_bounds() {
        let start = Instant::now();
        let mut window = TunnelWindow::new(start);
        let steady = Duration::from_secs(20);
        assert_eq!(window.observe(start, steady, 8, start + steady), None);
        assert_eq!(window.limit(8), TUNNEL_INITIAL_WINDOW);
    }

    #[test]
    fn one_slow_window_of_chunks_halves_once() {
        let start = Instant::now();
        let mut window = TunnelWindow::new(start);
        for _ in 0..6 {
            window.observe(start, Duration::from_secs(1), 8, start);
        }
        assert_eq!(window.limit(8), 8);
        // Eight chunks issued together all come back slow.
        let issued = start + Duration::from_secs(1);
        let slow = Duration::from_secs(40);
        let done = issued + slow;
        assert_eq!(window.observe(issued, slow, 8, done), Some(4));
        for _ in 0..7 {
            assert_eq!(window.observe(issued, slow, 8, done), None);
        }
        // Chunks issued under the smaller window, still slow: halve again,
        // down to one and no further.
        let issued = done + Duration::from_secs(1);
        let done = issued + slow;
        assert_eq!(window.observe(issued, slow, 8, done), Some(2));
        let issued = done + Duration::from_secs(1);
        let done = issued + slow;
        assert_eq!(window.observe(issued, slow, 8, done), Some(1));
        let issued = done + Duration::from_secs(1);
        assert_eq!(window.observe(issued, slow, 8, issued + slow), None);
        assert_eq!(window.limit(8), 1);
    }
}
