// SPDX-License-Identifier: GPL-3.0-or-later
//! Filesystem helpers, ported from `file.utils.ts` and the copy/unzip/remove IPC handlers.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

#[cfg(unix)]
const DEFAULT_FILE_MODE: u32 = 0o644;

#[cfg(unix)]
fn set_mode(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(DEFAULT_FILE_MODE));
}

#[cfg(not(unix))]
fn set_mode(_p: &Path) {}

/// Recursive copy that merges into and overwrites `dest`.
pub fn copy_dir(src: &Path, dest: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            set_mode(&target);
        }
    }
    Ok(())
}

pub fn remove_path(p: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Extract a zip, refusing entries that would escape `out` (upstream has no zip-slip check).
pub fn unzip(zip_path: &Path, out: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading zip archive")?;
    std::fs::create_dir_all(out)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            tracing::warn!("skipping unsafe zip entry {}", entry.name());
            continue;
        };
        let target = out.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&target)?;
        io::copy(&mut entry, &mut f)?;
        set_mode(&target);
    }
    Ok(out.to_path_buf())
}

/// Top-level directories of a folder, minus `__MACOSX`.
pub fn list_dirs(p: &Path) -> io::Result<Vec<String>> {
    let mut out: Vec<String> = std::fs::read_dir(p)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "__MACOSX")
        .collect();
    out.sort();
    Ok(out)
}

/// Newest modification time of any file under `p` (`getLatestDirUpdateTime`).
pub fn latest_mtime(p: &Path) -> Option<DateTime<Utc>> {
    walkdir::WalkDir::new(p)
        .into_iter()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .map(DateTime::<Utc>::from)
}

pub fn mtime(p: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(p).ok()?.modified().ok().map(DateTime::<Utc>::from)
}

/// Total size in bytes of everything under `p`.
pub fn dir_size(p: &Path) -> u64 {
    walkdir::WalkDir::new(p).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// Lowercase ascii slug, like the npm `slug` package for download file names.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "addon".into()
    } else {
        out
    }
}

/// Reveal a path in the platform file manager.
pub fn open_in_file_manager(p: &Path) -> io::Result<()> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(cmd).arg(p).spawn().map(|_| ())
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> io::Result<()> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(cmd).arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Deadly Boss Mods (DBM)"), "deadly-boss-mods-dbm");
        assert_eq!(slug("!!!"), "addon");
    }

    #[test]
    fn unzip_blocks_traversal() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("a.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        let opts: zip::write::SimpleFileOptions = Default::default();
        w.start_file("Good/Good.toc", opts).unwrap();
        w.write_all(b"## Title: Good").unwrap();
        w.start_file("../evil.txt", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        let out = dir.path().join("out");
        unzip(&zp, &out).unwrap();
        assert!(out.join("Good/Good.toc").exists());
        assert!(!dir.path().join("evil.txt").exists());
    }
}
