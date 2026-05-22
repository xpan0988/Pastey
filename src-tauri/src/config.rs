use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{crypto, error::AppResult, logging, models::AppConfig, storage::AppPaths};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredConfig {
    pub version: u32,
    pub default_expiry_minutes: u64,
    pub inbox_dir: Option<String>,
    pub auto_burn_after_download: bool,
    #[serde(default)]
    pub speed_limit_mbps: Option<f64>,
    pub shortcut: String,
    pub app_secret: String,
    #[serde(default)]
    pub device_id: String,
}

pub fn load_or_create(paths: &AppPaths, shortcut: &str) -> AppResult<StoredConfig> {
    if paths.config_path.exists() {
        let content = fs::read_to_string(&paths.config_path)?;
        let mut stored: StoredConfig = serde_json::from_str(&content)?;
        let mut changed = false;
        if stored.device_id.trim().is_empty() {
            stored.device_id = uuid::Uuid::new_v4().to_string();
            changed = true;
        }
        if stored.version < 3 {
            stored.version = 3;
            changed = true;
        }
        if changed {
            save(paths, &stored)?;
        }
        logging::write_transfer_line(&format!(
            "[pastey settings][backend] event=config_loaded speed_limit_mbps={:?}",
            normalize_speed_limit(stored.speed_limit_mbps)
        ));
        return Ok(stored);
    }

    let stored = StoredConfig {
        version: 3,
        default_expiry_minutes: 15,
        inbox_dir: None,
        auto_burn_after_download: false,
        speed_limit_mbps: None,
        shortcut: shortcut.to_string(),
        app_secret: crypto::encode_key(&crypto::random_key()),
        device_id: uuid::Uuid::new_v4().to_string(),
    };

    save(paths, &stored)?;
    logging::write_transfer_line(
        "[pastey settings][backend] event=config_created speed_limit_mbps=None",
    );
    Ok(stored)
}

pub fn save(paths: &AppPaths, config: &StoredConfig) -> AppResult<()> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&paths.config_path, json)?;
    logging::write_transfer_line(&format!(
        "[pastey settings][backend] event=config_persisted speed_limit_mbps={:?}",
        normalize_speed_limit(config.speed_limit_mbps)
    ));
    Ok(())
}

pub fn public_config(paths: &AppPaths, config: &StoredConfig) -> AppConfig {
    AppConfig {
        default_expiry_minutes: clamp_expiry(config.default_expiry_minutes),
        inbox_dir: Some(effective_inbox_dir(paths, config).display().to_string()),
        auto_burn_after_download: config.auto_burn_after_download,
        speed_limit_mbps: normalize_speed_limit(config.speed_limit_mbps),
        shortcut: config.shortcut.clone(),
        app_data_path: paths.app_data_dir.display().to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

pub fn update(
    paths: &AppPaths,
    current: &mut StoredConfig,
    incoming: AppConfig,
) -> AppResult<AppConfig> {
    logging::write_transfer_line(&format!(
        "[pastey settings][backend] event=config_update_received speed_limit_mbps={:?}",
        incoming.speed_limit_mbps
    ));
    current.default_expiry_minutes = clamp_expiry(incoming.default_expiry_minutes);
    current.auto_burn_after_download = incoming.auto_burn_after_download;
    current.inbox_dir = normalize_inbox_dir(paths, incoming.inbox_dir.as_deref());
    current.speed_limit_mbps = normalize_speed_limit(incoming.speed_limit_mbps);
    current.version = 3;
    save(paths, current)?;
    Ok(public_config(paths, current))
}

pub fn effective_inbox_dir(paths: &AppPaths, config: &StoredConfig) -> std::path::PathBuf {
    match config.inbox_dir.as_deref() {
        Some(path) if !path.trim().is_empty() => Path::new(path).to_path_buf(),
        _ => paths.inbox_dir.clone(),
    }
}

pub fn master_key(config: &StoredConfig) -> AppResult<[u8; 32]> {
    crypto::decode_key(&config.app_secret)
}

fn clamp_expiry(value: u64) -> u64 {
    match value {
        5 | 15 | 60 | 1440 => value,
        _ => 15,
    }
}

fn normalize_inbox_dir(paths: &AppPaths, value: Option<&str>) -> Option<String> {
    let Some(path) = value else {
        return None;
    };

    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == paths.inbox_dir.display().to_string() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_speed_limit(value: Option<f64>) -> Option<f64> {
    let value = value?;
    if !value.is_finite() || value <= 0.0 {
        None
    } else {
        Some(value.clamp(1.0, 10_000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::AppPaths;

    fn test_paths() -> AppPaths {
        let root = std::env::temp_dir().join(format!("pastey_config_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        AppPaths {
            app_data_dir: root.clone(),
            db_path: root.join("db.sqlite"),
            payloads_dir: root.join("payloads"),
            inbox_dir: root.join("inbox"),
            temp_dir: root.join("temp"),
            logs_dir: root.join("logs"),
            config_path: root.join("config.json"),
        }
    }

    #[test]
    fn speed_limit_normalization_treats_invalid_values_as_unlimited() {
        assert_eq!(normalize_speed_limit(None), None);
        assert_eq!(normalize_speed_limit(Some(0.0)), None);
        assert_eq!(normalize_speed_limit(Some(-10.0)), None);
        assert_eq!(normalize_speed_limit(Some(f64::NAN)), None);
    }

    #[test]
    fn speed_limit_normalization_accepts_positive_values() {
        assert_eq!(normalize_speed_limit(Some(10.0)), Some(10.0));
        assert_eq!(normalize_speed_limit(Some(50.0)), Some(50.0));
        assert_eq!(normalize_speed_limit(Some(100.0)), Some(100.0));
        assert_eq!(normalize_speed_limit(Some(20_000.0)), Some(10_000.0));
    }

    #[test]
    fn speed_limit_update_is_persisted() {
        let paths = test_paths();
        let mut stored = load_or_create(&paths, "Ctrl+Shift+V").unwrap();
        let incoming = public_config(&paths, &stored);
        let updated = update(
            &paths,
            &mut stored,
            AppConfig {
                speed_limit_mbps: Some(50.0),
                ..incoming
            },
        )
        .unwrap();
        let reloaded = load_or_create(&paths, "Ctrl+Shift+V").unwrap();

        assert_eq!(updated.speed_limit_mbps, Some(50.0));
        assert_eq!(reloaded.speed_limit_mbps, Some(50.0));

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }
}
