// SPDX-License-Identifier: GPL-3.0-or-later
//! Addon folder fingerprints, ported from `wowup-folder-scanner.ts`, `curse-folder-scanner.ts`,
//! `folder-scan-context.ts` and the native `curse.cc` hash.
//!
//! Both scanners select the same set of files: the folder's own toc files, `Bindings.xml`, and
//! everything those include (recursively through xml), then hash each file and hash the sorted
//! concatenation of the per-file hashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use regex::Regex;
use walkdir::WalkDir;

use crate::models::AddonScanResult;

const TOC_SUFFIXES: &[&str] =
    &["mainline", "bcc", "tbc", "classic", "vanilla", "wrath", "wotlkc", "cata", "mists", "forever", "camelot"];

fn is_whitespace(b: u8) -> bool {
    b == 9 || b == 10 || b == 13 || b == 32
}

/// CurseForge's MurmurHash2 variant: whitespace (tab, LF, CR, space) is skipped and the seed is 1.
pub fn curse_murmur2(buf: &[u8]) -> u32 {
    const M: u32 = 1_540_483_477;
    let normalized_len = buf.iter().filter(|b| !is_whitespace(**b)).count() as u32;
    let mut h: u32 = 1 ^ normalized_len;
    let mut k: u32 = 0;
    let mut shift: u32 = 0;
    for &b in buf {
        if is_whitespace(b) {
            continue;
        }
        k |= (b as u32) << shift;
        shift += 8;
        if shift == 32 {
            let k1 = k.wrapping_mul(M);
            let k2 = (k1 ^ (k1 >> 24)).wrapping_mul(M);
            h = h.wrapping_mul(M) ^ k2;
            k = 0;
            shift = 0;
        }
    }
    if shift > 0 {
        h = (h ^ k).wrapping_mul(M);
    }
    let h1 = (h ^ (h >> 13)).wrapping_mul(M);
    h1 ^ (h1 >> 15)
}

pub fn md5_hex(buf: &[u8]) -> String {
    hex::encode(Md5::digest(buf))
}

const INVALID_PATH_CHARS: &[char] = &[
    '|', '\0', '\u{1}', '\u{2}', '\u{3}', '\u{4}', '\u{5}', '\u{6}', '\u{8}', '\t', '\n', '\u{b}', '\u{c}', '\r',
    '\u{e}', '\u{f}', '\u{10}', '\u{11}', '\u{12}', '\u{13}', '\u{14}', '\u{15}', '\u{16}', '\u{17}', '\u{18}',
    '\u{19}', '\u{1a}', '\u{1b}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}',
];

/// Which fingerprints to compute for a folder.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashesNeeded {
    pub wowup: bool,
    pub curseforge: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FolderScanResults {
    pub wowup: Option<AddonScanResult>,
    pub curseforge: Option<AddonScanResult>,
}

struct Patterns {
    toc_comments: Regex,
    toc_include: Regex,
    xml_comments: Regex,
    xml_include: Regex,
}

fn patterns() -> &'static Patterns {
    static P: std::sync::OnceLock<Patterns> = std::sync::OnceLock::new();
    P.get_or_init(|| Patterns {
        toc_comments: Regex::new(r"(?m)\s*#.*$").unwrap(),
        // `(?<!\.\.)` from the original is enforced by rejecting `..` after matching.
        toc_include: Regex::new(r"(?i)^\s*(.+\.(?:xml|lua))\s*$").unwrap(),
        xml_comments: Regex::new(r"(?s)<!--.*?-->").unwrap(),
        xml_include: Regex::new(r#"(?i)<(?:Include|Script)\s+file=["']([^"']+)["']\s*/>"#).unwrap(),
    })
}

/// `^([^/]+)[\\/]\1([-_](suffix))?\.toc$` against the path relative to the AddOns folder.
fn is_root_toc(rel_lower: &str) -> bool {
    let Some((folder, file)) = rel_lower.split_once(['/', '\\']) else { return false };
    if file.contains(['/', '\\']) {
        return false;
    }
    let Some(stem) = file.strip_suffix(".toc") else { return false };
    if stem == folder {
        return true;
    }
    stem.strip_prefix(folder)
        .and_then(|rest| rest.strip_prefix(['-', '_']))
        .is_some_and(|suffix| TOC_SUFFIXES.contains(&suffix))
}

fn is_root_bindings(rel_lower: &str) -> bool {
    match rel_lower.split_once(['/', '\\']) {
        Some((_, file)) => file == "bindings.xml",
        None => false,
    }
}

/// Walks one addon folder and computes the requested fingerprints from a single read per file.
pub struct FolderScanner {
    folder: PathBuf,
    /// lowercase path -> real path, to cope with authors whose includes differ in case.
    file_map: HashMap<String, PathBuf>,
    cache: HashMap<PathBuf, Vec<u8>>,
}

impl FolderScanner {
    pub fn new(folder: &Path) -> Self {
        let mut file_map = HashMap::new();
        for entry in WalkDir::new(folder).follow_links(true).into_iter().flatten() {
            if entry.file_type().is_file() {
                let p = entry.path().to_path_buf();
                file_map.insert(p.to_string_lossy().to_lowercase(), p);
            }
        }
        Self { folder: folder.to_path_buf(), file_map, cache: HashMap::new() }
    }

    fn read(&mut self, path: &Path) -> std::io::Result<&[u8]> {
        if !self.cache.contains_key(path) {
            let data = std::fs::read(path)?;
            self.cache.insert(path.to_path_buf(), data);
        }
        Ok(self.cache.get(path).map(|v| v.as_slice()).unwrap_or_default())
    }

    fn real_path(&self, path: &Path) -> Option<PathBuf> {
        let normalized = normalize(path);
        self.file_map.get(&normalized.to_string_lossy().to_lowercase()).cloned()
    }

    fn matching_files(&mut self, curse_style: bool) -> Vec<PathBuf> {
        let parent = self.folder.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let parent_lower = format!("{}{}", parent.to_string_lossy().to_lowercase(), std::path::MAIN_SEPARATOR);
        let mut matching: Vec<PathBuf> = Vec::new();
        let mut tocs: Vec<PathBuf> = Vec::new();
        let mut all: Vec<PathBuf> = self.file_map.values().cloned().collect();
        all.sort();
        for path in all {
            let lower = path.to_string_lossy().to_lowercase();
            let rel = lower.strip_prefix(&parent_lower).unwrap_or(&lower);
            if is_root_toc(rel) {
                tocs.push(path);
            } else if is_root_bindings(rel) {
                matching.push(path);
            }
        }
        for toc in tocs {
            self.process_include(&mut matching, &toc, curse_style);
        }
        matching
    }

    fn process_include(&mut self, matching: &mut Vec<PathBuf>, file: &Path, curse_style: bool) {
        let Some(real) = self.real_path(file) else { return };
        if matching.contains(&real) {
            return;
        }
        matching.push(real.clone());
        let ext = real.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext != "xml" && ext != "toc" {
            return;
        }
        let Ok(bytes) = self.read(&real) else { return };
        let text = String::from_utf8_lossy(bytes).into_owned();
        let p = patterns();
        let includes: Vec<String> = if ext == "xml" {
            let text = p.xml_comments.replace_all(&text, "");
            if curse_style {
                // The CurseForge scanner matches line by line (a quirk of the original .NET regex).
                text.split('\n')
                    .filter_map(|l| p.xml_include.captures(l.trim()).map(|c| c[1].trim().to_string()))
                    .collect()
            } else {
                p.xml_include.captures_iter(&text).map(|c| c[1].to_string()).collect()
            }
        } else {
            let text = p.toc_comments.replace_all(&text, "");
            text.split('\n')
                .filter_map(|l| p.toc_include.captures(l).map(|c| c[1].trim().to_string()))
                .collect()
        };
        let dir = real.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for include in includes {
            if include.contains(INVALID_PATH_CHARS) {
                break;
            }
            if include.contains("..") {
                continue;
            }
            let rel = include.replace('\\', std::path::MAIN_SEPARATOR_STR);
            self.process_include(matching, &dir.join(rel), curse_style);
        }
    }

    fn sorted(mut files: Vec<PathBuf>) -> Vec<PathBuf> {
        files.sort_by_key(|f| f.to_string_lossy().to_lowercase());
        files
    }

    pub fn scan(&mut self, needed: HashesNeeded) -> FolderScanResults {
        let folder_name = self.folder.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let path = self.folder.to_string_lossy().into_owned();
        let mut results = FolderScanResults::default();

        if needed.wowup {
            let files = Self::sorted(self.matching_files(false));
            let mut hashes: Vec<String> =
                files.iter().filter_map(|f| self.read(f).ok().map(md5_hex)).collect();
            hashes.sort();
            let fingerprint = md5_hex(hashes.concat().as_bytes());
            results.wowup = Some(AddonScanResult {
                source: "wowup".into(),
                file_count: files.len(),
                fingerprint,
                fingerprint_num: 0,
                folder_name: folder_name.clone(),
                path: path.clone(),
            });
        }

        if needed.curseforge {
            let files = Self::sorted(self.matching_files(true));
            let mut hashes: Vec<u32> =
                files.iter().filter_map(|f| self.read(f).ok().map(curse_murmur2)).collect();
            hashes.sort();
            let concat: String = hashes.iter().map(|h| h.to_string()).collect();
            let fp = curse_murmur2(concat.as_bytes());
            results.curseforge = Some(AddonScanResult {
                source: "curseforge".into(),
                file_count: files.len(),
                fingerprint: fp.to_string(),
                fingerprint_num: fp,
                folder_name,
                path,
            });
        }
        results
    }
}

/// Resolve `.` components without touching the filesystem (path.join semantics).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Scan many folders in parallel on the blocking pool.
pub async fn scan_folders(folders: Vec<PathBuf>, needed: HashesNeeded) -> Vec<(PathBuf, FolderScanResults)> {
    use futures::stream::{self, StreamExt};
    stream::iter(folders)
        .map(|f| async move {
            let f2 = f.clone();
            let res = tokio::task::spawn_blocking(move || FolderScanner::new(&f2).scan(needed))
                .await
                .unwrap_or_default();
            (f, res)
        })
        .buffer_unordered(4)
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn murmur_matches_reference() {
        // Whitespace is ignored entirely, including in the length seed.
        assert_eq!(curse_murmur2(b""), curse_murmur2(b" \t\r\n"));
        assert_eq!(curse_murmur2(b"a b"), curse_murmur2(b"ab"));
        assert_ne!(curse_murmur2(b"ab"), curse_murmur2(b"ba"));
        // Standard MurmurHash2 with seed 1 on input without whitespace.
        assert_eq!(curse_murmur2(b"abcd"), murmur2_reference(b"abcd", 1));
        assert_eq!(curse_murmur2(b"abcdefg"), murmur2_reference(b"abcdefg", 1));
    }

    fn murmur2_reference(data: &[u8], seed: u32) -> u32 {
        const M: u32 = 0x5bd1_e995;
        let mut h = seed ^ data.len() as u32;
        let chunks = data.chunks_exact(4);
        let rem = chunks.remainder();
        for c in chunks {
            let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            k = k.wrapping_mul(M);
            k ^= k >> 24;
            k = k.wrapping_mul(M);
            h = h.wrapping_mul(M) ^ k;
        }
        if !rem.is_empty() {
            for (i, b) in rem.iter().enumerate() {
                h ^= (*b as u32) << (8 * i);
            }
            h = h.wrapping_mul(M);
        }
        h ^= h >> 13;
        h = h.wrapping_mul(M);
        h ^ (h >> 15)
    }

    #[test]
    fn root_toc_matching() {
        assert!(is_root_toc("myaddon/myaddon.toc"));
        assert!(is_root_toc("myaddon/myaddon_mainline.toc"));
        assert!(is_root_toc("myaddon/myaddon-classic.toc"));
        assert!(!is_root_toc("myaddon/other.toc"));
        assert!(!is_root_toc("myaddon/sub/myaddon.toc"));
        assert!(is_root_bindings("myaddon/bindings.xml"));
    }

    #[test]
    fn scans_includes() {
        let dir = tempfile::tempdir().unwrap();
        let addon = dir.path().join("MyAddon");
        std::fs::create_dir_all(addon.join("libs")).unwrap();
        std::fs::write(addon.join("MyAddon.toc"), "## Title: x\n# comment.lua\ncore.lua\nlibs\\libs.xml\n").unwrap();
        std::fs::write(addon.join("core.lua"), "print(1)").unwrap();
        std::fs::write(addon.join("unused.lua"), "print(2)").unwrap();
        std::fs::write(addon.join("libs/libs.xml"), "<Ui><Script file=\"a.lua\"/><!-- <Script file=\"b.lua\"/> --></Ui>")
            .unwrap();
        std::fs::write(addon.join("libs/a.lua"), "a").unwrap();
        std::fs::write(addon.join("libs/b.lua"), "b").unwrap();
        let r = FolderScanner::new(&addon).scan(HashesNeeded { wowup: true, curseforge: true });
        assert_eq!(r.wowup.as_ref().unwrap().file_count, 4);
        assert_eq!(r.curseforge.as_ref().unwrap().file_count, 4);
        assert_eq!(r.wowup.unwrap().fingerprint.len(), 32);
    }
}
