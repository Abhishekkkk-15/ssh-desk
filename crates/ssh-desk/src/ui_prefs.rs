//! Persistent UI preferences (`~/.config/ssh-desk/config.toml`).

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::session::config_dir;
use crate::theme::{self, ThemeId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub theme: ThemeId,
    #[serde(default)]
    pub compact_dock: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeId::Dark,
            compact_dock: false,
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load() -> UiConfig {
    let Ok(path) = config_path() else {
        return UiConfig::default();
    };
    if !path.exists() {
        return UiConfig::default();
    }
    match fs::read_to_string(&path) {
        Ok(raw) => match toml::from_str::<UiConfig>(&raw) {
            Ok(cfg) => {
                theme::set_theme(cfg.theme);
                cfg
            }
            Err(e) => {
                warn!(error = %e, "corrupt config.toml · using defaults");
                UiConfig::default()
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to read config.toml");
            UiConfig::default()
        }
    }
}

pub fn save(cfg: &UiConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = config_path()?;
    let raw = toml::to_string_pretty(cfg).context("serialize config.toml")?;
    let mut f = fs::File::create(&path).with_context(|| format!("write {}", path.display()))?;
    f.write_all(raw.as_bytes())?;
    Ok(())
}
