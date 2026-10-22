//! Presets: named project looks saved as JSON files in the config folder
//! (`presets/`), so they can be shared by copying the file. A preset holds
//! the whole edit configuration; applying one keeps the project's own
//! timeline (see `EditConfig::apply_preset`).

use crate::edit::EditConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub settings: EditConfig,
    #[serde(skip)]
    pub path: PathBuf,
}

pub fn dir() -> PathBuf {
    crate::util::config_dir().join("presets")
}

/// Every preset, by name ("1. Youtube" before "2. TikTok").
pub fn list() -> Vec<Preset> {
    let mut out: Vec<Preset> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let mut p: Preset = serde_json::from_str(&text).ok()?;
            p.path = path;
            Some(p)
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

impl Preset {
    /// Saves a new preset; the file name follows the preset name.
    pub fn create(name: &str, settings: &EditConfig) -> Result<Self> {
        std::fs::create_dir_all(dir())?;
        let stem: String = name.chars().map(|c| if c.is_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect();
        let stem = if stem.trim().is_empty() { "Preset".to_string() } else { stem.trim().to_string() };
        let mut path = dir().join(format!("{stem}.json"));
        let mut n = 2;
        while path.exists() {
            path = dir().join(format!("{stem} {n}.json"));
            n += 1;
        }
        let p = Preset { name: name.to_string(), settings: settings.clone(), path };
        p.save()?;
        Ok(p)
    }

    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.path, text).with_context(|| format!("writing {}", self.path.display()))
    }

    pub fn delete(&self) -> Result<()> {
        std::fs::remove_file(&self.path).with_context(|| format!("deleting {}", self.path.display()))
    }
}
