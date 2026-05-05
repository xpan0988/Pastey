use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    crypto,
    error::AppResult,
    models::AppConfig,
    storage::AppPaths,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredConfig {
    pub version: u32,
    pub default_expiry_minutes: u64,
    pub inbox_dir: Option<String>,
    pub auto_burn_after_download: bool,
    pub shortcut: String,
    pub app_secret: String,
}

pub fn load_or_create(paths: &AppPaths, shortcut: &str) -> AppResult<StoredConfig> {
    if paths.config_path.exists() {
        let content = fs::read_to_string(&paths.config_path)?;
        let stored: StoredConfig = serde_json::from_str(&content)?;
        return Ok(stored);
    }

    let stored = StoredConfig {
        version: 1,
        default_expiry_minutes: 15,
        inbox_dir: None,
        auto_burn_after_download: false,
        shortcut: shortcut.to_string(),
        app_secret: crypto::encode_key(&crypto::random_key()),
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
    AppConfig {
        default_expiry_minutes: clamp_expiry(config.default_expiry_minutes),
        inbox_dir: Some(effective_inbox_dir(paths, config).display().to_string()),
        auto_burn_after_download: config.auto_burn_after_download,
        shortcut: config.shortcut.clone(),
        app_data_path: paths.app_data_dir.display().to_string(),
    }
}

pub fn update(paths: &AppPaths, current: &mut StoredConfig, incoming: AppConfig) -> AppResult<AppConfig> {
    current.default_expiry_minutes = clamp_expiry(incoming.default_expiry_minutes);
    current.auto_burn_after_download = incoming.auto_burn_after_download;
    current.inbox_dir = normalize_inbox_dir(paths, incoming.inbox_dir.as_deref());
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
