// SPDX-License-Identifier: GPL-3.0-or-later
//! TOC file parsing and per-client toc selection, ported from `toc.service.ts` and `toc.utils.ts`.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::models::{AddonFolder, Toc, WowClientType, WowGameType};

const SUFFIXES: &[&str] =
    &["classic", "vanilla", "bcc", "tbc", "mainline", "wrath", "wotlkc", "cata", "mists", "forever", "camelot"];

fn game_type_names(g: WowGameType) -> &'static [&'static str] {
    match g {
        WowGameType::Retail => &["standard", "mainline"],
        WowGameType::Forever => &["forever", "camelot", "mainline"],
        WowGameType::Classic => &["vanilla", "classic"],
        WowGameType::BurningCrusade => &["tbc", "classic"],
        WowGameType::Wotlk => &["wrath", "classic"],
        WowGameType::Cata => &["cata", "classic"],
        WowGameType::Mists => &["mists", "classic"],
    }
}

fn is_known_game_type_name(name: &str) -> bool {
    name == "plunderstorm" || name == "wowhack" || WowGameType::ALL.iter().any(|g| game_type_names(*g).contains(&name))
}

fn load_condition_regex() -> Regex {
    Regex::new(r"(?i)\[(Allow|Exclude)LoadGameType\s+([^\]]*)\]").unwrap()
}

/// Remove `[AllowLoad...]`/`[ExcludeLoad...]` directives from a value.
pub fn strip_load_conditions(s: &str) -> String {
    if !s.contains('[') {
        return s.to_string();
    }
    Regex::new(r"(?i)\s*\[(Allow|Exclude)Load[^\]]*\]").unwrap().replace_all(s, "").trim().to_string()
}

pub fn has_load_game_type_conditions(line: &str) -> bool {
    load_condition_regex().is_match(line)
}

/// Whether the inline load conditions on a toc line apply to the game type. An allow list of only
/// unknown names is satisfied, like the client does.
pub fn is_load_condition_met(line: &str, game_type: WowGameType) -> bool {
    let names = game_type_names(game_type);
    for cap in load_condition_regex().captures_iter(line) {
        let kind = cap[1].to_ascii_lowercase();
        let listed: Vec<String> = cap[2]
            .split(',')
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| is_known_game_type_name(n))
            .collect();
        let matches = listed.iter().any(|n| names.contains(&n.as_str()));
        if kind == "allow" && !listed.is_empty() && !matches {
            return false;
        }
        if kind == "exclude" && matches {
            return false;
        }
    }
    true
}

fn strip_encoded_chars(value: &str) -> String {
    let mut s = strip_load_conditions(value);
    if s.contains("|c") {
        s = Regex::new(r"(?i)(\|c[a-z0-9]{8})|(\|r)").unwrap().replace_all(&s, "").trim().to_string();
    }
    if s.contains("|T") {
        s = Regex::new(r"(\|T.*\|t)").unwrap().replace_all(&s, "").trim().to_string();
    }
    s.replace("|r", "")
}

fn get_value(key: &str, text: &str) -> String {
    let re = Regex::new(&format!(r"(?m)^## {}:(.*?)\r?$", regex::escape(key))).unwrap();
    re.captures(text).map(|c| strip_encoded_chars(c[1].trim())).unwrap_or_default()
}

fn opt(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn titles_by_game_type(text: &str) -> Option<BTreeMap<String, String>> {
    let title_re = Regex::new(r"(?i)^##\s*Title\s*:").unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| title_re.is_match(l)).collect();
    if !lines.iter().any(|l| has_load_game_type_conditions(l)) {
        return None;
    }
    let mut titles = BTreeMap::new();
    for g in WowGameType::ALL {
        if let Some(line) = lines.iter().rev().find(|l| is_load_condition_met(l, g)) {
            let t = strip_encoded_chars(title_re.replace(line, "").trim());
            if !t.is_empty() {
                titles.insert(g.as_str().to_string(), t);
            }
        }
    }
    Some(titles)
}

/// Parse toc text. `file_path` is only recorded, never read.
pub fn parse_toc_text(file_path: &Path, text: &str) -> Toc {
    let text = text.trim_start_matches('\u{feff}').trim();
    let deps = get_value("Dependencies", text);
    let req = get_value("RequiredDeps", text);
    let dependency_list: Vec<String> =
        deps.split(',').chain(req.split(',')).map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
    let mut interface: Vec<String> = Vec::new();
    for v in get_value("Interface", text).split(',').map(|x| x.trim().to_string()) {
        if !interface.contains(&v) {
            interface.push(v);
        }
    }
    let website = {
        let w = get_value("Website", text);
        if w.is_empty() {
            get_value("X-Website", text)
        } else {
            w
        }
    };
    Toc {
        file_name: file_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
        file_path: file_path.to_string_lossy().into_owned(),
        interface,
        title: opt(get_value("Title", text)),
        titles: titles_by_game_type(text),
        author: opt(get_value("Author", text)),
        website: opt(website),
        version: opt(get_value("Version", text)),
        part_of: opt(get_value("X-Part-Of", text)),
        category: opt(get_value("X-Category", text)),
        localizations: opt(get_value("X-Localizations", text)),
        dependencies: opt(if deps.is_empty() { req } else { deps }),
        curse_project_id: opt(get_value("X-Curse-Project-ID", text)),
        wow_interface_id: opt(get_value("X-WoWI-ID", text)),
        wago_addon_id: opt(get_value("X-Wago-ID", text)),
        tuk_ui_project_id: opt(get_value("X-Tukui-ProjectID", text)),
        tuk_ui_project_folders: opt(get_value("X-Tukui-ProjectFolders", text)),
        load_on_demand: opt(get_value("LoadOnDemand", text)),
        dependency_list,
        addon_provider: opt(get_value("X-AddonProvider", text)),
        notes: opt(get_value("Notes", text)),
    }
}

pub fn parse_toc(path: &Path) -> std::io::Result<Toc> {
    let bytes = std::fs::read(path)?;
    Ok(parse_toc_text(path, &String::from_utf8_lossy(&bytes)))
}

/// All `.toc` files directly inside an addon folder, parsed.
pub fn read_folder_tocs(dir: &Path) -> Vec<Toc> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut tocs: Vec<Toc> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("toc")))
        .filter_map(|e| parse_toc(&e.path()).ok())
        .collect();
    tocs.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    tocs
}

fn suffix_of(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".toc")?;
    let idx = stem.rfind(['-', '_'])?;
    let suffix = &stem[idx + 1..];
    SUFFIXES.contains(&suffix).then(|| suffix.to_string())
}

/// Pick the toc file name the given client would load (`getTocForGameType`).
pub fn toc_for_game_type(names: &[String], client: WowClientType) -> Option<String> {
    use WowClientType::*;
    let wanted: &[&str] = match client {
        Beta | Retail | RetailPtr | RetailXPtr => &["mainline"],
        ClassicEra | ClassicEraPtr => &["classic", "vanilla"],
        Classic | ClassicPtr => &["mists"],
        Anniversary => &["tbc", "bcc"],
        Forever | ClassicBeta => &["forever", "camelot"],
        None => &[],
    };
    names
        .iter()
        .find(|n| suffix_of(n).is_some_and(|s| wanted.contains(&s.as_str())))
        .or_else(|| names.iter().find(|n| n.to_ascii_lowercase().ends_with(".toc") && suffix_of(n).is_none()))
        .cloned()
}

/// The toc for a folder as the given client sees it (`getTocForGameType2`).
pub fn toc_for_folder(folder: &AddonFolder, client: WowClientType) -> Option<Toc> {
    let names: Vec<String> = folder.tocs.iter().map(|t| t.file_name.clone()).collect();
    let toc = match toc_for_game_type(&names, client) {
        Some(name) => folder.tocs.iter().find(|t| t.file_name == name),
        None => folder.tocs.iter().find(|t| {
            t.file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&t.file_name).eq_ignore_ascii_case(&folder.name)
        }),
    }?;
    let mut toc = toc.clone();
    if let Some(title) = toc.titles.as_ref().and_then(|t| t.get(client.game_type().as_str())) {
        toc.title = Some(title.clone());
    }
    Some(toc)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOC: &str = "## Interface: 110002, 11507\n## Title: |cff00ff00My|r Addon\n## Author: Someone\n## Version: 1.2.3\n## X-WoWI-ID: 1234\n## Dependencies: LibStub, Ace3\n## X-Website: https://x\n\nmain.lua\n";

    #[test]
    fn parses_fields() {
        let toc = parse_toc_text(Path::new("/a/MyAddon/MyAddon.toc"), TOC);
        assert_eq!(toc.title.as_deref(), Some("My Addon"));
        assert_eq!(toc.interface, vec!["110002", "11507"]);
        assert_eq!(toc.version.as_deref(), Some("1.2.3"));
        assert_eq!(toc.wow_interface_id.as_deref(), Some("1234"));
        assert_eq!(toc.dependency_list, vec!["LibStub", "Ace3"]);
        assert_eq!(toc.website.as_deref(), Some("https://x"));
        assert_eq!(toc.file_name, "MyAddon.toc");
    }

    #[test]
    fn selects_toc() {
        let names: Vec<String> = ["A.toc", "A_Mainline.toc", "A-Classic.toc", "A_Mists.toc"].map(String::from).into();
        assert_eq!(toc_for_game_type(&names, WowClientType::Retail).unwrap(), "A_Mainline.toc");
        assert_eq!(toc_for_game_type(&names, WowClientType::ClassicEra).unwrap(), "A-Classic.toc");
        assert_eq!(toc_for_game_type(&names, WowClientType::Classic).unwrap(), "A_Mists.toc");
        assert_eq!(toc_for_game_type(&names, WowClientType::Anniversary).unwrap(), "A.toc");
    }

    #[test]
    fn load_conditions() {
        let line = "## Title: Foo [AllowLoadGameType wrath, classic]";
        assert!(is_load_condition_met(line, WowGameType::Classic));
        assert!(!is_load_condition_met(line, WowGameType::Retail));
        assert_eq!(strip_load_conditions("Foo [AllowLoadGameType wrath]"), "Foo");
    }
}
