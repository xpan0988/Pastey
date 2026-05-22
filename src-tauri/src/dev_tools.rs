const DEV_TOOLS_ENV: &str = "PASTEY_DEV_TOOLS";

pub fn is_dev_tools_enabled() -> bool {
    cfg!(debug_assertions) || env_flag_enabled(DEV_TOOLS_ENV)
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| env_value_enabled(&value))
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
}
