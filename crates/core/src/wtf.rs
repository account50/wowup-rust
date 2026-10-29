// SPDX-License-Identifier: GPL-3.0-or-later
//! WTF folder backups and the WTF explorer, ported from `wtf.service.ts`.
//! Backups are zips of the whole WTF folder with a `wowup-meta.json` manifest, compatible with
//! WowUp's own backups.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil;
use crate::models::WowInstallation;

const META_FILE: &str = "wowup-meta.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetaEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMeta {
    pub created_by: String,
    pub created_at: i64,
    pub contents: Vec<MetaEntry>,
}

#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub file_name: String,
    pub location: PathBuf,
    pub size: u64,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

pub fn wtf_path(inst: &WowInstallation) -> PathBuf {
    Path::new(&inst.location).join("WTF")
}

pub fn backup_dir(root: &Path, inst: &WowInstallation) -> PathBuf {
    root.join("wtf_backups").join(&inst.id)
}

/// Flattened tree with sha256 hashes; a directory hashes the concatenation of its children.
fn tree(base: &Path, p: &Path, out: &mut Vec<MetaEntry>) -> Result<(String, u64)> {
    let rel = format!("{}{}", std::path::MAIN_SEPARATOR, p.strip_prefix(base).unwrap_or(Path::new("")).to_string_lossy());
    if p.is_dir() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(p)?.flatten().map(|e| e.path()).collect();
        children.sort();
        let mut hasher = Sha256::new();
        let mut size = 0;
        for c in children {
            if c.file_name().is_some_and(|n| n == META_FILE) {
                continue;
            }
            let (h, s) = tree(base, &c, out)?;
            hasher.update(h.as_bytes());
            size += s;
        }
        let hash = hex::encode(hasher.finalize());
        out.push(MetaEntry { path: rel, is_directory: true, size, hash: hash.clone() });
        Ok((hash, size))
    } else {
        let bytes = std::fs::read(p)?;
        let hash = hex::encode(Sha256::digest(&bytes));
        out.push(MetaEntry { path: rel, is_directory: false, size: bytes.len() as u64, hash: hash.clone() });
        Ok((hash, bytes.len() as u64))
    }
}

pub fn create_backup(root: &Path, inst: &WowInstallation) -> Result<PathBuf> {
    let wtf = wtf_path(inst);
    if !wtf.is_dir() {
        bail!("WTF folder not found at {}", wtf.display());
    }
    let dir = backup_dir(root, inst);
    std::fs::create_dir_all(&dir)?;
    let mut contents = Vec::new();
    tree(&wtf, &wtf, &mut contents)?;
    let now = chrono::Utc::now().timestamp_millis();
    let meta = BackupMeta { created_by: "manual".into(), created_at: now, contents };
    let out = dir.join(format!("wtf_{now}.zip"));
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&out)?);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in walkdir::WalkDir::new(&wtf).into_iter().flatten() {
        let rel = entry.path().strip_prefix(&wtf).unwrap_or(entry.path());
        if rel.as_os_str().is_empty() {
            continue;
        }
        let name = rel.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            zw.add_directory(format!("{name}/"), opts)?;
        } else {
            zw.start_file(name, opts)?;
            let mut f = std::fs::File::open(entry.path())?;
            std::io::copy(&mut f, &mut zw)?;
        }
    }
    zw.start_file(META_FILE, opts)?;
    zw.write_all(&serde_json::to_vec_pretty(&meta)?)?;
    zw.finish()?;
    Ok(out)
}

fn read_meta(zip_path: &Path) -> Result<BackupMeta> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut f = archive.by_name(META_FILE).context("backup has no wowup-meta.json")?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(serde_json::from_str(&s)?)
}

pub fn list_backups(root: &Path, inst: &WowInstallation) -> Vec<BackupInfo> {
    let Ok(rd) = std::fs::read_dir(backup_dir(root, inst)) else { return vec![] };
    let mut out: Vec<BackupInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
        .map(|e| {
            let p = e.path();
            let meta = read_meta(&p);
            BackupInfo {
                file_name: e.file_name().to_string_lossy().into_owned(),
                size: e.metadata().map(|m| m.len()).unwrap_or(0),
                created_at: meta.as_ref().ok().and_then(|m| chrono::DateTime::from_timestamp_millis(m.created_at)),
                error: meta.err().map(|e| format!("{e:#}")),
                location: p,
            }
        })
        .collect();
    out.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    out
}

pub fn delete_backup(root: &Path, inst: &WowInstallation, file_name: &str) -> Result<()> {
    let p = backup_dir(root, inst).join(file_name);
    if !p.exists() || file_name.contains(['/', '\\']) {
        bail!("path not found");
    }
    std::fs::remove_file(p)?;
    Ok(())
}

/// Keep only the newest `keep` backups.
pub fn prune_backups(root: &Path, inst: &WowInstallation, keep: usize) {
    for b in list_backups(root, inst).into_iter().skip(keep) {
        let _ = std::fs::remove_file(b.location);
    }
}

/// Restore a backup over WTF, keeping the current WTF until the restore validates (`applyBackup`).
pub fn apply_backup(root: &Path, inst: &WowInstallation, file_name: &str) -> Result<()> {
    if !file_name.ends_with(".zip") || file_name.contains(['/', '\\']) {
        bail!("invalid backup file");
    }
    let zip = backup_dir(root, inst).join(file_name);
    if !zip.exists() {
        bail!("backup not found");
    }
    let meta = read_meta(&zip)?;
    let wtf = wtf_path(inst);
    let old = Path::new(&inst.location).join("WTF-wowup");
    let _ = fsutil::remove_path(&old);
    if wtf.exists() {
        std::fs::rename(&wtf, &old)?;
    }
    let result = (|| -> Result<()> {
        fsutil::unzip(&zip, &wtf)?;
        fsutil::remove_path(&wtf.join(META_FILE))?;
        let mut now = Vec::new();
        tree(&wtf, &wtf, &mut now)?;
        let count = |v: &[MetaEntry]| v.iter().filter(|e| !e.path.ends_with(META_FILE)).count();
        if count(&now) != count(&meta.contents) {
            bail!("restored file count does not match backup");
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            let _ = fsutil::remove_path(&old);
            Ok(())
        }
        Err(e) => {
            let _ = fsutil::remove_path(&wtf);
            if old.exists() {
                std::fs::rename(&old, &wtf)?;
            }
            Err(e)
        }
    }
}

/// One node of the WTF explorer tree.
#[derive(Debug, Clone)]
pub struct WtfNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub depth: usize,
    /// Backup copies and Blizzard files, shown dimmed.
    pub ignorable: bool,
    /// SavedVariables for an addon that is not installed.
    pub orphaned: bool,
}

pub fn explore(inst: &WowInstallation, installed_folders: &[String]) -> Vec<WtfNode> {
    let wtf = wtf_path(inst);
    let lower: Vec<String> = installed_folders.iter().map(|f| f.to_lowercase()).collect();
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(&wtf).min_depth(1).sort_by_file_name().into_iter().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let lname = name.to_lowercase();
        let is_dir = e.file_type().is_dir();
        let in_saved_vars = e.path().components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("SavedVariables"));
        let is_lua = lname.ends_with(".lua");
        let ignorable = lname.ends_with(".lua.bak") || lname.starts_with("blizzard_");
        let orphaned = in_saved_vars && is_lua && !ignorable && !lower.contains(&lname.trim_end_matches(".lua").to_string());
        out.push(WtfNode {
            size: if is_dir { fsutil::dir_size(e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) },
            name,
            path: e.path().to_path_buf(),
            is_dir,
            depth: e.depth() - 1,
            ignorable,
            orphaned,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let client = dir.path().join("_retail_");
        let wtf = client.join("WTF/Account/ME/SavedVariables");
        std::fs::create_dir_all(&wtf).unwrap();
        std::fs::write(wtf.join("Foo.lua"), "Foo = {}").unwrap();
        let inst = WowInstallation {
            id: "i".into(),
            client_type: crate::models::WowClientType::Retail,
            location: client.to_string_lossy().into(),
            label: String::new(),
            display_name: String::new(),
            selected: true,
            default_addon_channel_type: Default::default(),
            default_auto_update: false,
            available_update_count: 0,
        };
        let root = dir.path().join("data");
        let b = create_backup(&root, &inst).unwrap();
        assert!(b.exists());
        std::fs::write(wtf.join("Foo.lua"), "Foo = { changed = true }").unwrap();
        let list = list_backups(&root, &inst);
        assert_eq!(list.len(), 1);
        assert!(list[0].error.is_none(), "{:?}", list[0].error);
        apply_backup(&root, &inst, &list[0].file_name).unwrap();
        assert_eq!(std::fs::read_to_string(wtf.join("Foo.lua")).unwrap(), "Foo = {}");
        let nodes = explore(&inst, &[]);
        assert!(nodes.iter().any(|n| n.name == "Foo.lua" && n.orphaned));
    }
}
