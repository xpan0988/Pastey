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
}
