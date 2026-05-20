// config.rs — user-facing settings persisted at ~/.config/todarchy/config.toml.
//
// For v0.2 the only setting is `sync_folder`, the directory where
// tasks.automerge is mirrored for cross-device sync (typically an iCloud
// Drive / Dropbox / Syncthing path). Empty / missing = local-only mode.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Absolute path to a directory that the OS syncs across devices
    /// (iCloud, Dropbox, Syncthing, etc.). todarchy writes
    /// `<sync_folder>/tasks.automerge` on every save and watches it for
    /// external changes.
    #[serde(default)]
    pub sync_folder: String,

    /// Unix millis of the last successful read-or-write against the sync
    /// folder. `None` means we've never synced (or sync is off).
    #[serde(default, rename = "last_synced_at")]
    pub last_synced_at: Option<i64>,

    /// Human-readable reason the last sync attempt failed, if any.
    /// Cleared on next successful sync.
    #[serde(default, rename = "last_sync_error")]
    pub last_sync_error: Option<String>,

    /// HTTP relay base URL (e.g. `https://sync.example.com`). Empty
    /// when server-sync mode is off. Mutually exclusive with
    /// `sync_folder` in practice — the UI gates that, but the runtime
    /// tolerates both being set (server takes precedence as a remote
    /// mirror; the folder still works as a local cache).
    #[serde(default)]
    pub server_base_url: String,

    /// Server-side doc id for the main `tasks.automerge` bytes. Must be
    /// the SAME across all of a user's devices using this server, or
    /// each device sees its own isolated remote doc. Defaults to a
    /// fresh `main_<22-char>` id on first server-mode setup.
    #[serde(default)]
    pub server_main_doc_id: String,
}

/// Three explicit sync transports. The runtime allows server + folder
/// to coexist, but the UI presents them as alternatives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMode {
    LocalOnly,
    Folder(PathBuf),
    Server { base_url: String, main_doc_id: String },
}

/// Generate a fresh main-doc id matching the iOS format:
/// `main_<22 base64url-no-pad chars>` carrying 16 bytes of entropy.
/// 22 chars is exactly ceil(16 * 8 / 6) without padding — what the iOS
/// generator emits — so a docs id minted on one platform is shaped the
/// same as one minted on the other.
pub fn generate_main_doc_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("main_{b64}")
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
    Ok(sync_folder()?.map(|f| f.join("tasks.automerge")))
}

/// Return the sync folder path if configured (and non-empty). Used by
/// the shared-project subsystem to locate `shared_<id>.automerge.enc`
/// files as siblings of `tasks.automerge`.
pub fn sync_folder() -> Result<Option<PathBuf>> {
    let cfg = load()?;
    if cfg.sync_folder.trim().is_empty() {
        return Ok(None);
    }
    let folder = Path::new(&cfg.sync_folder).to_path_buf();
    if !folder.exists() {
        std::fs::create_dir_all(&folder).ok();
    }
    Ok(Some(folder))
}

/// Server-relay config if both fields are populated. Returns the pair
/// already trimmed of whitespace so callers don't have to re-validate.
pub fn server_config() -> Result<Option<(String, String)>> {
    let cfg = load()?;
    let base = cfg.server_base_url.trim().to_string();
    let id = cfg.server_main_doc_id.trim().to_string();
    if base.is_empty() || id.is_empty() {
        return Ok(None);
    }
    Ok(Some((base, id)))
}
