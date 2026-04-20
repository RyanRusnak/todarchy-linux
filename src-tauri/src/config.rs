// config.rs — user-facing settings persisted at ~/.config/todarchy/config.toml.
//
// For v0.2 the only setting is `sync_folder`, the directory where
// tasks.automerge is mirrored for cross-device sync (typically an iCloud
// Drive / Dropbox / Syncthing path). Empty / missing = local-only mode.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Absolute path to a directory that the OS syncs across devices
    /// (iCloud, Dropbox, Syncthing, etc.). todarchy writes
    /// `<sync_folder>/tasks.automerge` on every save and watches it for
    /// external changes.
    #[serde(default)]
    pub sync_folder: String,
}

fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .or_else(dirs::home_dir)
        .context("no config dir")?
        .join("todarchy");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let text = toml::to_string_pretty(cfg)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Return the resolved `<sync_folder>/tasks.automerge` path if sync is
/// configured. The folder is created on demand.
pub fn sync_doc_path() -> Result<Option<PathBuf>> {
    let cfg = load()?;
    if cfg.sync_folder.trim().is_empty() {
        return Ok(None);
    }
    let folder = Path::new(&cfg.sync_folder);
    if !folder.exists() {
        std::fs::create_dir_all(folder).ok();
    }
    Ok(Some(folder.join("tasks.automerge")))
}
