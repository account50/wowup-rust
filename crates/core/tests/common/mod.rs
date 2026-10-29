// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared helpers for provider integration tests against mock HTTP servers.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wowup_core::addons::AddonService;
use wowup_core::installations::Installations;
use wowup_core::models::{WowClientType, WowInstallation};
use wowup_core::storage::Storage;

pub struct Sandbox {
    pub _dir: tempfile::TempDir,
    pub svc: Arc<AddonService>,
    pub inst: WowInstallation,
}

impl Sandbox {
    pub fn addons_dir(&self) -> PathBuf {
        self.inst.addons_path()
    }
}

/// An addon folder whose toc carries `version`.
pub fn write_addon(addons: &Path, name: &str, version: &str) {
    let d = addons.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{name}.toc")), format!("## Interface: 110002\n## Title: {name}\n## Version: {version}\n\n{name}.lua\n"))
        .unwrap();
    std::fs::write(d.join(format!("{name}.lua")), format!("-- {name} {version}\nprint('{name}')\n")).unwrap();
}

/// A zip of addon folders, each at `version`.
pub fn addon_zip(folders: &[&str], version: &str) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let o: zip::write::SimpleFileOptions = Default::default();
        for name in folders {
            w.start_file(format!("{name}/{name}.toc"), o).unwrap();
            w.write_all(format!("## Interface: 110002\n## Title: {name}\n## Version: {version}\n\n{name}.lua\n").as_bytes())
                .unwrap();
            w.start_file(format!("{name}/{name}.lua"), o).unwrap();
            w.write_all(format!("-- {name} {version}\nprint('{name}')\n").as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

/// A data dir + retail client with only the given providers enabled, so tests never hit the network.
pub fn sandbox(enabled: &[&str], configure: impl FnOnce(&mut wowup_core::preferences::Preferences)) -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let client = dir.path().join("wow/_retail_");
    std::fs::create_dir_all(client.join("Interface/AddOns")).unwrap();
    let storage = Arc::new(Storage::open(dir.path().join("data")).unwrap());
    let mut prefs = storage.preferences().unwrap();
    for (k, v) in prefs.provider_enabled.iter_mut() {
        *v = enabled.contains(&k.as_str());
    }
    configure(&mut prefs);
    storage.save_preferences(&prefs).unwrap();
    let inst = Installations::new(&storage).add(&client, WowClientType::Retail, "").unwrap();
    let svc = AddonService::new(storage).unwrap();
    Sandbox { _dir: dir, svc, inst }
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tests in one binary share process env (the mock base URLs), so they take turns.
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
