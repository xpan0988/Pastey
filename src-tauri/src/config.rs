use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    crypto, dev_tools,
    error::AppResult,
    models::{default_save_received_to_inbox, AppConfig},
    storage::AppPaths,
    transfer_tuning,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredConfig {
    pub version: u32,
    pub default_expiry_minutes: u64,
    pub inbox_dir: Option<String>,
    pub auto_burn_after_download: bool,
    #[serde(default = "default_save_received_to_inbox")]
    pub save_received_files_to_inbox: bool,
    #[serde(default = "default_save_received_to_inbox")]
    pub save_received_images_to_inbox: bool,
    #[serde(default)]
    pub transfer_window_override: Option<usize>,
    #[serde(default = "dev_tools::default_dev_tools_enabled")]
    pub dev_tools_enabled: bool,
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
        if stored.version < 4 {
            stored.version = 4;
            changed = true;
        }
        if changed {
            save(paths, &stored)?;
        }
        return Ok(stored);
    }

    let stored = StoredConfig {
        version: 4,
        default_expiry_minutes: 15,
        inbox_dir: None,
        auto_burn_after_download: false,
        save_received_files_to_inbox: true,
        save_received_images_to_inbox: true,
        transfer_window_override: None,
        dev_tools_enabled: dev_tools::default_dev_tools_enabled(),
        shortcut: shortcut.to_string(),
        app_secret: crypto::encode_key(&crypto::random_key()),
        device_id: uuid::Uuid::new_v4().to_string(),
    };

    save(paths, &stored)?;
    Ok(stored)
}

pub fn save(paths: &AppPaths, config: &StoredConfig) -> AppResult<()> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&paths.config_path, json)?;
    Ok(())
}

pub fn public_config(paths: &AppPaths, config: &StoredConfig) -> AppConfig {
    public_config_with_dev_tools(
        paths,
        config,
        dev_tools::effective_dev_tools_enabled(config.dev_tools_enabled),
    )
}

fn public_config_with_dev_tools(
    paths: &AppPaths,
    config: &StoredConfig,
    dev_tools_enabled: bool,
) -> AppConfig {
    AppConfig {
        default_expiry_minutes: clamp_expiry(config.default_expiry_minutes),
        inbox_dir: Some(effective_inbox_dir(paths, config).display().to_string()),
        auto_burn_after_download: config.auto_burn_after_download,
        save_received_files_to_inbox: config.save_received_files_to_inbox,
        save_received_images_to_inbox: config.save_received_images_to_inbox,
        transfer_window_override: if dev_tools_enabled {
            transfer_tuning::normalize_transfer_window_override(config.transfer_window_override)
        } else {
            None
        },
        dev_tools_enabled,
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
    let was_effective_dev_tools_enabled =
        dev_tools::effective_dev_tools_enabled(current.dev_tools_enabled);
    current.dev_tools_enabled = incoming.dev_tools_enabled;
    let effective_dev_tools_enabled =
        dev_tools::effective_dev_tools_enabled(current.dev_tools_enabled);
    update_with_dev_tools_enabled(
        paths,
        current,
        incoming,
        effective_dev_tools_enabled,
        was_effective_dev_tools_enabled,
    )
}

fn update_with_dev_tools_enabled(
    paths: &AppPaths,
    current: &mut StoredConfig,
    incoming: AppConfig,
    dev_tools_enabled: bool,
    was_dev_tools_enabled: bool,
) -> AppResult<AppConfig> {
    current.default_expiry_minutes = clamp_expiry(incoming.default_expiry_minutes);
    current.auto_burn_after_download = incoming.auto_burn_after_download;
    current.save_received_files_to_inbox = incoming.save_received_files_to_inbox;
    current.save_received_images_to_inbox = incoming.save_received_images_to_inbox;
    current.inbox_dir = normalize_inbox_dir(paths, incoming.inbox_dir.as_deref());
    if dev_tools_enabled && (was_dev_tools_enabled || incoming.transfer_window_override.is_some()) {
        current.transfer_window_override =
            transfer_tuning::normalize_transfer_window_override(incoming.transfer_window_override);
    }
    current.version = 4;
    save(paths, current)?;
    Ok(public_config_with_dev_tools(
        paths,
        current,
        dev_tools_enabled,
    ))
}

pub fn effective_inbox_dir(paths: &AppPaths, config: &StoredConfig) -> std::path::PathBuf {
    match config.inbox_dir.as_deref() {
        Some(path) if !path.trim().is_empty() => Path::new(path).to_path_buf(),
        _ => paths.inbox_dir.clone(),
    }
}

pub fn received_item_destination_dir(
    paths: &AppPaths,
    config: &StoredConfig,
    mime_type: Option<&str>,
) -> std::path::PathBuf {
    if should_save_received_to_inbox(config, mime_type) {
        effective_inbox_dir(paths, config)
    } else {
        paths.temp_dir.clone()
    }
}

pub fn should_save_received_to_inbox(config: &StoredConfig, mime_type: Option<&str>) -> bool {
    if mime_type
        .map(|value| value.trim().to_ascii_lowercase().starts_with("image/"))
        .unwrap_or(false)
    {
        config.save_received_images_to_inbox
    } else {
        config.save_received_files_to_inbox
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
    fn old_config_with_speed_limit_deserializes_safely() {
        let stored: StoredConfig = serde_json::from_str(
            r#"{
                "version": 3,
                "default_expiry_minutes": 15,
                "inbox_dir": null,
                "auto_burn_after_download": false,
                "speed_limit_mbps": 50,
                "shortcut": "Ctrl+Shift+V",
                "app_secret": "abc",
                "device_id": "device"
            }"#,
        )
        .unwrap();

        assert_eq!(stored.transfer_window_override, None);
        assert!(stored.save_received_files_to_inbox);
        assert!(stored.save_received_images_to_inbox);
        assert_eq!(
            stored.dev_tools_enabled,
            dev_tools::default_dev_tools_enabled()
        );
    }

    #[test]
    fn received_inbox_persistence_defaults_to_enabled() {
        let paths = test_paths();
        let stored = load_or_create(&paths, "Ctrl+Shift+V").unwrap();
        let public = public_config_with_dev_tools(&paths, &stored, false);

        assert!(stored.save_received_files_to_inbox);
        assert!(stored.save_received_images_to_inbox);
        assert!(public.save_received_files_to_inbox);
        assert!(public.save_received_images_to_inbox);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn received_inbox_persistence_roundtrips() {
        let paths = test_paths();
        let mut stored = load_or_create(&paths, "Ctrl+Shift+V").unwrap();
        let incoming = public_config_with_dev_tools(&paths, &stored, false);

        let updated = update_with_dev_tools_enabled(
            &paths,
            &mut stored,
            AppConfig {
                save_received_files_to_inbox: false,
                save_received_images_to_inbox: true,
                ..incoming
            },
            false,
            false,
        )
        .unwrap();
        let reloaded = load_or_create(&paths, "Ctrl+Shift+V").unwrap();

        assert!(!updated.save_received_files_to_inbox);
        assert!(updated.save_received_images_to_inbox);
        assert!(!reloaded.save_received_files_to_inbox);
        assert!(reloaded.save_received_images_to_inbox);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn received_item_destination_honors_file_and_image_toggles() {
        let paths = test_paths();
        let stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: false,
            save_received_images_to_inbox: true,
            transfer_window_override: None,
            dev_tools_enabled: false,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };

        assert_eq!(
            received_item_destination_dir(&paths, &stored, Some("application/pdf")),
            paths.temp_dir
        );
        assert_eq!(
            received_item_destination_dir(&paths, &stored, Some("image/png")),
            paths.inbox_dir
        );

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn transfer_window_update_is_persisted_when_dev_tools_are_enabled() {
        let paths = test_paths();
        let mut stored = load_or_create(&paths, "Ctrl+Shift+V").unwrap();
        let incoming = public_config_with_dev_tools(&paths, &stored, true);
        let updated = update_with_dev_tools_enabled(
            &paths,
            &mut stored,
            AppConfig {
                transfer_window_override: Some(8),
                ..incoming
            },
            true,
            true,
        )
        .unwrap();
        let reloaded = load_or_create(&paths, "Ctrl+Shift+V").unwrap();

        assert_eq!(updated.transfer_window_override, Some(8));
        assert_eq!(reloaded.transfer_window_override, Some(8));

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn normal_public_config_hides_dev_transfer_window() {
        let paths = test_paths();
        let stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: Some(16),
            dev_tools_enabled: false,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };

        let public = public_config_with_dev_tools(&paths, &stored, false);

        assert_eq!(public.transfer_window_override, None);
        assert!(!public.dev_tools_enabled);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn stored_dev_tools_setting_is_visible_in_public_config() {
        let paths = test_paths();
        let stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: Some(8),
            dev_tools_enabled: true,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };

        let public = public_config_with_dev_tools(&paths, &stored, true);

        assert!(public.dev_tools_enabled);
        assert_eq!(public.transfer_window_override, Some(8));

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn dev_tools_toggle_is_persisted() {
        let paths = test_paths();
        let mut stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: None,
            dev_tools_enabled: false,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };
        save(&paths, &stored).unwrap();
        let incoming = public_config_with_dev_tools(&paths, &stored, false);

        let updated = update(
            &paths,
            &mut stored,
            AppConfig {
                dev_tools_enabled: true,
                ..incoming
            },
        )
        .unwrap();
        let reloaded = load_or_create(&paths, "Ctrl+Shift+V").unwrap();

        assert!(updated.dev_tools_enabled);
        assert!(reloaded.dev_tools_enabled);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn diagnostics_stay_hidden_when_dev_tools_are_disabled() {
        let paths = test_paths();
        let stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: Some(16),
            dev_tools_enabled: false,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };

        let public = public_config_with_dev_tools(&paths, &stored, false);

        assert!(!public.dev_tools_enabled);
        assert_eq!(public.transfer_window_override, None);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn release_mode_can_show_diagnostics_when_dev_tools_are_enabled() {
        let paths = test_paths();
        let stored = StoredConfig {
            version: 4,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: None,
            dev_tools_enabled: true,
            shortcut: "Ctrl+Shift+V".into(),
            app_secret: "abc".into(),
            device_id: "device".into(),
        };

        let public = public_config_with_dev_tools(&paths, &stored, true);

        assert!(public.dev_tools_enabled);

        let _ = fs::remove_dir_all(paths.app_data_dir);
    }
}
