// SPDX-License-Identifier: GPL-3.0-or-later
//! WoW installation detection and CRUD, ported from `warcraft-platform.*.ts`,
//! `warcraft-installation.service.ts` and `product-db.ts`.
//!
//! An installation's `location` here is the client folder (e.g. `.../World of Warcraft/_retail_`),
//! rather than the executable path upstream uses; `Interface/AddOns` and `WTF` live directly under it.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::models::{AddonChannelType, InstalledProduct, WowClientType, WowInstallation};
use crate::storage::Storage;

/// Battle.net agent product database location for this platform.
pub fn product_db_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(PathBuf::from("/Users/Shared/Battle.net/Agent/product.db"));
    }
    if cfg!(windows) {
        for drive in 'C'..='Z' {
            let p = PathBuf::from(format!("{drive}:\\ProgramData\\Battle.net\\Agent\\product.db"));
            if p.exists() {
                return Some(p);
            }
        }
        return None;
    }
    // Linux: Lutris prefixes.
    let home = dirs::home_dir()?;
    let cfg = std::fs::read_to_string(home.join(".config/lutris/system.yml")).ok()?;
    let game_path = regex::Regex::new(r"game_path: (.*)").ok()?.captures(&cfg)?[1].trim().to_string();
    for prefix in ["battlenet/drive_c", "world-of-warcraft/drive_c", "world-of-warcraft-classic/drive_c"] {
        let p = Path::new(&game_path).join(prefix).join("ProgramData/Battle.net/Agent/product.db");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

// --- Minimal protobuf reader for product.db -------------------------------------------------------

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

/// Iterate (field number, length-delimited payload) pairs, skipping other wire types.
fn fields(buf: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(tag) = read_varint(buf, &mut pos) else { break };
        let (field, wire) = (tag >> 3, tag & 7);
        match wire {
            0 => {
                if read_varint(buf, &mut pos).is_none() {
                    break;
                }
            }
            1 => pos += 8,
            2 => {
                let Some(len) = read_varint(buf, &mut pos) else { break };
                let end = pos + len as usize;
                if end > buf.len() {
                    break;
                }
                out.push((field, &buf[pos..end]));
                pos = end;
            }
            5 => pos += 4,
            _ => break,
        }
    }
    out
}

fn string_field(buf: &[u8], n: u64) -> Option<String> {
    fields(buf).into_iter().find(|(f, _)| *f == n).map(|(_, b)| String::from_utf8_lossy(b).into_owned())
}

/// Decode product.db: products with family `wow`, keyed to client folders.
pub fn decode_product_db(buf: &[u8]) -> Vec<InstalledProduct> {
    let mut out: Vec<InstalledProduct> = Vec::new();
    for (field, product) in fields(buf) {
        if field != 1 {
            continue;
        }
        if string_field(product, 6).as_deref() != Some("wow") {
            continue;
        }
        let name = string_field(product, 1).unwrap_or_default();
        let Some(client) = fields(product).into_iter().find(|(f, _)| *f == 3).map(|(_, b)| b) else { continue };
        let location = string_field(client, 1).unwrap_or_default();
        let folder = string_field(client, 13).unwrap_or_default();
        let Some(client_type) = WowClientType::from_folder_name(&folder) else { continue };
        out.retain(|p| p.client_type != client_type);
        let location = Path::new(&location.replace('\\', std::path::MAIN_SEPARATOR_STR)).join(&folder);
        out.push(InstalledProduct { name, location: location.to_string_lossy().into_owned(), client_type });
    }
    out
}

/// Products installed through Battle.net, plus any client folders found in the default install
/// directory (covers installs whose agent db is missing).
pub fn detect_products() -> Vec<InstalledProduct> {
    let mut found = product_db_path()
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| decode_product_db(&b))
        .unwrap_or_default();
    let defaults: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Applications/World of Warcraft")]
    } else if cfg!(windows) {
        vec![PathBuf::from(r"C:\Program Files (x86)\World of Warcraft"), PathBuf::from(r"C:\Program Files\World of Warcraft")]
    } else {
        vec![]
    };
    for root in defaults {
        let extra: Vec<InstalledProduct> =
            scan_root(&root).into_iter().filter(|p| !found.iter().any(|f| f.client_type == p.client_type)).collect();
        found.extend(extra);
    }
    found.into_iter().filter(|p| Path::new(&p.location).is_dir()).collect()
}

/// Client folders (`_retail_`, `_classic_era_`, ...) directly inside a WoW root folder.
pub fn scan_root(root: &Path) -> Vec<InstalledProduct> {
    let Ok(entries) = std::fs::read_dir(root) else { return vec![] };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            WowClientType::from_folder_name(&name).map(|ct| InstalledProduct {
                name: format!("wow{name}"),
                location: e.path().to_string_lossy().into_owned(),
                client_type: ct,
            })
        })
        .collect()
}

/// Work out the client type from a user-chosen path: a client folder, a game executable/app
/// bundle inside one, or the WoW root folder (picks `_retail_` first).
pub fn resolve_user_path(path: &Path) -> Result<(PathBuf, WowClientType)> {
    let mut p = path.to_path_buf();
    // Executable or .app inside the client folder.
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".app") || name.ends_with(".exe") {
        p = p.parent().map(Path::to_path_buf).unwrap_or(p);
    }
    for anc in p.ancestors() {
        if let Some(ct) = anc.file_name().and_then(|n| WowClientType::from_folder_name(&n.to_string_lossy())) {
            return Ok((anc.to_path_buf(), ct));
        }
    }
    let inner = scan_root(&p);
    if let Some(r) = inner.iter().find(|i| i.client_type == WowClientType::Retail).or(inner.first()) {
        return Ok((PathBuf::from(&r.location), r.client_type));
    }
    bail!("{} is not a World of Warcraft client folder", path.display())
}

fn normalize(p: &str) -> String {
    Path::new(p).components().collect::<PathBuf>().to_string_lossy().to_lowercase()
}

pub struct Installations<'a> {
    storage: &'a Storage,
}

impl<'a> Installations<'a> {
    pub fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    pub fn all(&self) -> Result<Vec<WowInstallation>> {
        let mut list = self.storage.installations()?;
        // Upstream migration: a Retail install that lives under _forever_ is a Forever install.
        let mut changed = false;
        for i in list.iter_mut() {
            if i.client_type == WowClientType::Retail && i.location.to_lowercase().contains("_forever_") {
                i.client_type = WowClientType::Forever;
                changed = true;
            }
        }
        if changed {
            self.storage.save_installations(&list)?;
        }
        Ok(list)
    }

    pub fn get(&self, id: &str) -> Result<Option<WowInstallation>> {
        Ok(self.all()?.into_iter().find(|i| i.id == id))
    }

    pub fn selected(&self) -> Result<Option<WowInstallation>> {
        let all = self.all()?;
        Ok(all.iter().find(|i| i.selected).cloned().or_else(|| all.first().cloned()))
    }

    fn display_name(existing: &[WowInstallation], ct: WowClientType, label: &str) -> String {
        let base = ct.display_name();
        if !label.is_empty() {
            return format!("{base} ({label})");
        }
        let same = existing.iter().filter(|i| i.client_type == ct).count();
        if same == 0 {
            base.to_string()
        } else {
            format!("{base} {}", same + 1)
        }
    }

    pub fn add(&self, location: &Path, client_type: WowClientType, label: &str) -> Result<WowInstallation> {
        let mut all = self.all()?;
        let loc = location.to_string_lossy().into_owned();
        if all.iter().any(|i| normalize(&i.location) == normalize(&loc)) {
            bail!("installation already exists: {loc}");
        }
        let inst = WowInstallation {
            id: uuid::Uuid::new_v4().to_string(),
            client_type,
            display_name: Self::display_name(&all, client_type, label),
            label: label.to_string(),
            location: loc,
            selected: all.is_empty(),
            default_addon_channel_type: AddonChannelType::Stable,
            default_auto_update: false,
            available_update_count: 0,
        };
        all.push(inst.clone());
        self.storage.save_installations(&all)?;
        Ok(inst)
    }

    /// Add every detected product not already present (startup auto-import / "Re-Scan").
    pub fn import_detected(&self) -> Result<Vec<WowInstallation>> {
        let mut added = Vec::new();
        for p in detect_products() {
            let existing = self.all()?;
            if existing.iter().any(|i| normalize(&i.location) == normalize(&p.location)) {
                continue;
            }
            added.push(self.add(Path::new(&p.location), p.client_type, "")?);
        }
        Ok(added)
    }

    pub fn update(&self, inst: &WowInstallation) -> Result<()> {
        let mut all = self.all()?;
        let Some(slot) = all.iter_mut().find(|i| i.id == inst.id) else { bail!("installation not found") };
        let mut inst = inst.clone();
        if inst.label != slot.label {
            let others: Vec<WowInstallation> = all.iter().filter(|i| i.id != inst.id).cloned().collect();
            inst.display_name = Self::display_name(&others, inst.client_type, &inst.label);
        }
        let slot = all.iter_mut().find(|i| i.id == inst.id).unwrap();
        *slot = inst;
        self.storage.save_installations(&all)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut all = self.all()?;
        let was_selected = all.iter().any(|i| i.id == id && i.selected);
        all.retain(|i| i.id != id);
        if was_selected {
            if let Some(first) = all.first_mut() {
                first.selected = true;
            }
        }
        self.storage.save_installations(&all)?;
        // Reconcile orphans: an installation's addon records go with it (folders are left alone).
        self.storage.remove_addons_for(id)
    }

    pub fn set_selected(&self, id: &str) -> Result<()> {
        let mut all = self.all()?;
        for i in all.iter_mut() {
            i.selected = i.id == id;
        }
        self.storage.save_installations(&all)
    }

    /// Move an installation up (-1) or down (+1) in the list.
    pub fn reorder(&self, id: &str, delta: i32) -> Result<()> {
        let mut all = self.all()?;
        let Some(idx) = all.iter().position(|i| i.id == id) else { bail!("installation not found") };
        let target = idx as i32 + delta;
        if target < 0 || target >= all.len() as i32 {
            return Ok(());
        }
        all.swap(idx, target as usize);
        self.storage.save_installations(&all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb_string(field: u64, s: &[u8]) -> Vec<u8> {
        let mut v = vec![((field << 3) | 2) as u8];
        v.push(s.len() as u8);
        v.extend_from_slice(s);
        v
    }

    #[test]
    fn decodes_product_db() {
        let mut client = pb_string(1, b"/Applications/World of Warcraft");
        client.extend(pb_string(13, b"_classic_era_"));
        let mut product = pb_string(1, b"wow_classic_era");
        product.extend(pb_string(2, b"wow_classic_era"));
        product.extend(pb_string(3, &client));
        product.extend(pb_string(6, b"wow"));
        let db = pb_string(1, &product);
        let products = decode_product_db(&db);
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].client_type, WowClientType::ClassicEra);
        assert!(products[0].location.ends_with("_classic_era_"));
    }

    #[test]
    fn resolves_paths() {
        let dir = tempfile::tempdir().unwrap();
        let retail = dir.path().join("_retail_");
        std::fs::create_dir_all(retail.join("World of Warcraft.app")).unwrap();
        let (p, ct) = resolve_user_path(&retail.join("World of Warcraft.app")).unwrap();
        assert_eq!(ct, WowClientType::Retail);
        assert_eq!(p, retail);
        let (p, _) = resolve_user_path(dir.path()).unwrap();
        assert_eq!(p, retail);
    }
}
