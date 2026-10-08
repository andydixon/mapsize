//! User settings from a JSON file in the OS-specific user configuration
//! directory. Knows nothing about rendering.

use crate::brand;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// User preferences. Load starts from the defaults and overlays the file.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub workers: String,         // conservative|balanced|aggressive|N
    pub follow_symlinks: String, // none|same-filesystem|all
    pub one_file_system: bool,
    pub units: String,     // iec|si
    pub size_mode: String, // allocated|logical
    pub theme: String,
    pub mouse: bool,
    pub excludes: Vec<String>,
    /// Maps an extension (without dot) to "#rrggbb".
    pub extension_colors: HashMap<String, String>,
    /// Maps a category name to "#rrggbb".
    pub category_colors: HashMap<String, String>,
    /// UI refresh rate while scanning.
    pub refresh_hz: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            workers: "balanced".into(),
            follow_symlinks: "none".into(),
            one_file_system: false,
            units: "iec".into(),
            size_mode: "allocated".into(),
            theme: "default".into(),
            mouse: true,
            excludes: Vec::new(),
            extension_colors: HashMap::new(),
            category_colors: HashMap::new(),
            refresh_hz: 10,
        }
    }
}

/// The config file location.
pub fn path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join(brand::NAME).join("config.json"))
}

/// Reads the config file if present. A missing file is not an error.
pub fn load() -> (Settings, Option<String>) {
    let Some(p) = path() else { return (Settings::default(), None) };
    let b = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
        Err(e) => return (Settings::default(), Some(e.to_string())),
    };
    match serde_json::from_slice::<Settings>(&b) {
        Ok(mut s) => {
            if !(1..=60).contains(&s.refresh_hz) {
                s.refresh_hz = 10;
            }
            (s, None)
        }
        Err(e) => (Settings::default(), Some(e.to_string())),
    }
}
