// config.rs — hand-owned settings at ~/.config/todarchy/config.toml.
//
// Omarchy-style: this file is *yours*. The app READS it (live — the sync
// watchers re-read every tick, so edits take effect within a second or two)
// but never rewrites it during normal use, so your comments and formatting
// survive. The only time the app touches it is `ensure_documented()` on
// startup, which creates the commented template on first run and does a
// one-time migration of older configs (stripping the runtime-state fields
// that used to live here). Sync status (last-synced time, last error) is now
// in-memory only — see sync.rs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Absolute path to a directory the OS syncs across devices (iCloud,
    /// Dropbox, Syncthing, …). Empty = no folder sync.
    #[serde(default)]
    pub sync_folder: String,

    /// HTTP relay base URL (self-hosted todarchy-server). Empty = no relay.
    #[serde(default)]
    pub server_base_url: String,

    /// Relay doc id for the main store — must match on all your devices.
    /// Mint one with `todarchy gen-id`.
    #[serde(default)]
    pub server_main_doc_id: String,
}

/// Sentinel in the documented template; its absence means the file is either
/// missing or in the old (app-written) format and should be migrated.
const HEADER: &str = "# todarchy configuration";

fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .or_else(dirs::home_dir)
        .context("no config dir")?
        .join("todarchy");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Read the config. Pure read — returns defaults if the file is missing.
pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    // Unknown keys (e.g. the retired last_synced_at/last_sync_error) are
    // ignored by serde, so old configs still load their sync settings.
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

/// Ensure a documented config file exists. Called once at startup. Creates
/// the commented template on first run; migrates an old app-written config to
/// the documented form once (preserving the user's sync settings). No-op if
/// the file already carries our header.
pub fn ensure_documented() -> Result<()> {
    let path = config_path()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(HEADER) {
        return Ok(()); // already documented — never rewrite (keep comments)
    }
    // First run → default template; migrating → preserve current settings.
    let cfg = if path.exists() { load()? } else { Config::default() };
    let text = documented(&cfg);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Render the commented config template with the given values interpolated.
fn documented(cfg: &Config) -> String {
    format!(
        r#"{HEADER} — edit this file by hand.
#
# Sync is off by default (local-only). Turn on either transport (or both) by
# filling in the values below. Edits take effect within a second or two — no
# restart needed. The app never rewrites this file, so your comments stay.

# A folder your OS keeps in sync across devices (Syncthing / Dropbox / iCloud).
# The app mirrors its store there and merges peers' changes automatically.
sync_folder = "{sync_folder}"

# A self-hosted todarchy-server relay (alternative or additional transport).
server_base_url = "{server}"

# Shared doc id for the relay — must be IDENTICAL on all your devices.
# Generate one with:  todarchy gen-id
server_main_doc_id = "{doc_id}"
"#,
        HEADER = HEADER,
        sync_folder = cfg.sync_folder,
        server = cfg.server_base_url,
        doc_id = cfg.server_main_doc_id,
    )
}

/// Generate a fresh main-doc id matching the iOS `main_<22 base64url>` format
/// (16 bytes of entropy). Printed by the `todarchy gen-id` subcommand.
pub fn generate_main_doc_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("main_{b64}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_template_round_trips() {
        // The commented template MUST parse back into the same values —
        // otherwise a migration could silently blank the user's sync config.
        let cfg = Config {
            sync_folder: "/home/me/Dropbox/todarchy_sync".into(),
            server_base_url: "https://sync.example.com".into(),
            server_main_doc_id: "main_ABC123".into(),
        };
        let text = documented(&cfg);
        assert!(text.contains(HEADER));
        let parsed: Config = toml::from_str(&text).expect("template must be valid TOML");
        assert_eq!(parsed.sync_folder, cfg.sync_folder);
        assert_eq!(parsed.server_base_url, cfg.server_base_url);
        assert_eq!(parsed.server_main_doc_id, cfg.server_main_doc_id);
    }

    #[test]
    fn old_flat_config_still_loads() {
        // Old configs carried retired keys; serde must ignore them, not fail.
        let old = r#"sync_folder = "/x"
last_synced_at = 123
last_sync_error = "oops"
server_base_url = "https://s"
server_main_doc_id = "main_y"
"#;
        let cfg: Config = toml::from_str(old).expect("old config must still parse");
        assert_eq!(cfg.sync_folder, "/x");
        assert_eq!(cfg.server_main_doc_id, "main_y");
    }
}

/// Resolved `<sync_folder>/tasks.automerge` if folder sync is configured.
pub fn sync_doc_path() -> Result<Option<PathBuf>> {
    Ok(sync_folder()?.map(|f| f.join("tasks.automerge")))
}

/// The sync folder path if configured (created on demand).
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

/// Relay config if both fields are set, trimmed.
pub fn server_config() -> Result<Option<(String, String)>> {
    let cfg = load()?;
    let base = cfg.server_base_url.trim().to_string();
    let id = cfg.server_main_doc_id.trim().to_string();
    if base.is_empty() || id.is_empty() {
        return Ok(None);
    }
    Ok(Some((base, id)))
}
