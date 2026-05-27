const DEV_TOOLS_ENV: &str = "PASTEY_DEV_TOOLS";

pub fn default_dev_tools_enabled() -> bool {
    // Debug builds keep the historical default-on behavior for development
    // sessions. Release builds default to hidden unless the user enables the
    // persisted Settings toggle or the environment force-enable is present.
    cfg!(debug_assertions)
}

pub fn effective_dev_tools_enabled(stored_enabled: bool) -> bool {
    effective_dev_tools_enabled_with_env(
        stored_enabled,
        std::env::var(DEV_TOOLS_ENV).ok().as_deref(),
    )
}

fn effective_dev_tools_enabled_with_env(stored_enabled: bool, env_value: Option<&str>) -> bool {
    stored_enabled || env_value.is_some_and(env_value_enabled)
}

fn env_value_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_value_accepts_common_enabled_values() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            assert!(env_value_enabled(value));
        }
    }

    #[test]
    fn env_value_rejects_disabled_values() {
        for value in ["0", "false", "", "no"] {
            assert!(!env_value_enabled(value));
        }
    }

    #[test]
    fn stored_setting_controls_dev_tools_when_env_is_absent() {
        assert!(effective_dev_tools_enabled_with_env(true, None));
        assert!(!effective_dev_tools_enabled_with_env(false, None));
    }

    #[test]
    fn env_value_force_enables_dev_tools() {
        assert!(effective_dev_tools_enabled_with_env(false, Some("1")));
        assert!(effective_dev_tools_enabled_with_env(false, Some("true")));
    }

    #[test]
    fn disabled_env_value_does_not_force_enable_dev_tools() {
        assert!(!effective_dev_tools_enabled_with_env(false, Some("0")));
        assert!(!effective_dev_tools_enabled_with_env(false, Some("false")));
    }
}
