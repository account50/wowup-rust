// SPDX-License-Identifier: GPL-3.0-or-later
//! JSON file storage, replacing the electron-store backed `addon-storage.service.ts`,
//! `preference-storage.service.ts` and the `wow_installations` key.
//!
//! Layout under the data dir (`~/Library/Application Support/WowUp.rs` on macOS):
//!   installations.json        list of WowInstallation
//!   preferences.json          Preferences
//!   addons/<installation>.json  list of Addon for that installation

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::models::{Addon, WowInstallation};
use crate::preferences::Preferences;

pub struct Storage {
    root: PathBuf,
    lock: Mutex<()>,
}

impl Storage {
    /// Default data dir, overridable with `WOWUP_DATA_DIR` (used by tests and sandboxes).
    pub fn default_root() -> PathBuf {
        if let Ok(dir) = std::env::var("WOWUP_DATA_DIR") {
            return PathBuf::from(dir);
        }
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("WowUp.rs")
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("addons"))
            .with_context(|| format!("creating data dir {}", root.display()))?;
        Ok(Self { root, lock: Mutex::new(()) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn read<T: DeserializeOwned + Default>(&self, path: &Path) -> Result<T> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write via a temp file and rename so a crash never leaves half a file behind.
    fn write<T: Serialize + ?Sized>(&self, path: &Path, value: &T) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn installations(&self) -> Result<Vec<WowInstallation>> {
        self.read(&self.root.join("installations.json"))
    }

    pub fn save_installations(&self, list: &[WowInstallation]) -> Result<()> {
        self.write(&self.root.join("installations.json"), list)
    }

    pub fn preferences(&self) -> Result<Preferences> {
        self.read(&self.root.join("preferences.json"))
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<()> {
        self.write(&self.root.join("preferences.json"), prefs)
    }

    fn addons_path(&self, installation_id: &str) -> PathBuf {
        self.root.join("addons").join(format!("{installation_id}.json"))
    }

    pub fn addons(&self, installation_id: &str) -> Result<Vec<Addon>> {
        self.read(&self.addons_path(installation_id))
    }

    pub fn save_addons(&self, installation_id: &str, addons: &[Addon]) -> Result<()> {
        self.write(&self.addons_path(installation_id), addons)
    }

    pub fn remove_addons_for(&self, installation_id: &str) -> Result<()> {
        let p = self.addons_path(installation_id);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
        Ok(())
    }

    /// Insert or replace an addon by id.
    pub fn upsert_addon(&self, installation_id: &str, addon: &Addon) -> Result<()> {
        let mut all = self.addons(installation_id)?;
        match all.iter_mut().find(|a| a.id == addon.id) {
            Some(existing) => *existing = addon.clone(),
            None => all.push(addon.clone()),
        }
        self.save_addons(installation_id, &all)
    }

    pub fn remove_addon(&self, installation_id: &str, addon_id: &str) -> Result<()> {
        let mut all = self.addons(installation_id)?;
        all.retain(|a| a.id != addon_id);
        self.save_addons(installation_id, &all)
    }
}
