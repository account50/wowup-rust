// SPDX-License-Identifier: GPL-3.0-or-later
//! Translations, using WowUp's own locale files (`locales/*.json`, copied from upstream
//! `src/assets/i18n`) and its keys, so upstream translation updates drop straight in.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// (code, display name, file contents), in upstream's language-picker order.
pub const LANGUAGES: [(&str, &str, &str); 13] = [
    ("en", "English", include_str!("../locales/en.json")),
    ("cs", "Čeština", include_str!("../locales/cs.json")),
    ("de", "Deutsch", include_str!("../locales/de.json")),
    ("es", "Español", include_str!("../locales/es.json")),
    ("fr", "Français", include_str!("../locales/fr.json")),
    ("it", "Italiano", include_str!("../locales/it.json")),
    ("pl", "Polski", include_str!("../locales/pl.json")),
    ("ko", "한국어", include_str!("../locales/ko.json")),
    ("nb", "Norsk bokmål", include_str!("../locales/nb.json")),
    ("pt", "Português", include_str!("../locales/pt.json")),
    ("ru", "Русский", include_str!("../locales/ru.json")),
    ("zh", "简体中文", include_str!("../locales/zh.json")),
    ("zh-TW", "繁體中文", include_str!("../locales/zh-TW.json")),
];

type Table = HashMap<String, String>;

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut Table) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        _ => {}
    }
}

fn load(code: &str) -> Table {
    let src = LANGUAGES.iter().find(|l| l.0 == code).map(|l| l.2).unwrap_or(LANGUAGES[0].2);
    let mut t = Table::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(src) {
        flatten("", &v, &mut t);
    }
    t
}

struct State {
    lang: Table,
    english: Table,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(State { lang: load("en"), english: load("en") }))
}

pub fn set_language(code: &str) {
    state().write().unwrap().lang = load(code);
}

/// Apply the language preference; empty follows the system locale.
pub fn apply(pref: &str) {
    if pref.is_empty() {
        set_language(&system_language());
    } else {
        set_language(pref);
    }
}

/// System locale mapped to a supported language (upstream falls back to the OS locale, then English).
pub fn system_language() -> String {
    let raw = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    let tag = raw.split('.').next().unwrap_or("").replace('_', "-");
    if tag.eq_ignore_ascii_case("zh-TW") || tag.eq_ignore_ascii_case("zh-HK") {
        return "zh-TW".into();
    }
    let base = tag.split('-').next().unwrap_or("").to_lowercase();
    let base = if base == "no" || base == "nn" { "nb".to_string() } else { base };
    if LANGUAGES.iter().any(|l| l.0 == base) {
        base
    } else {
        "en".into()
    }
}

/// Translate a key, falling back to English, then to the key itself.
pub fn t(key: &str) -> String {
    let s = state().read().unwrap();
    s.lang
        .get(key)
        .filter(|v| !v.is_empty())
        .or_else(|| s.english.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// Translate with `{name}` parameters and the ICU `{count, plural, =1{one} other{many}}` form upstream uses.
pub fn tp(key: &str, params: &[(&str, String)]) -> String {
    let mut s = t(key);
    for (name, value) in params {
        let plural = format!("{{{name}, plural,");
        while let Some(start) = s.find(&plural) {
            let Some(end) = matching_brace(&s, start) else { break };
            let body = &s[start + plural.len()..end];
            let pick = plural_branch(body, value);
            s.replace_range(start..=end, &pick);
        }
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}

fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn plural_branch(body: &str, value: &str) -> String {
    let mut branches: Vec<(String, String)> = Vec::new();
    let mut rest = body.trim();
    while let Some(open) = rest.find('{') {
        let label = rest[..open].trim().to_string();
        let Some(close) = matching_brace(rest, open) else { break };
        branches.push((label, rest[open + 1..close].to_string()));
        rest = rest[close + 1..].trim();
    }
    let exact = format!("={value}");
    let one = value == "1";
    branches
        .iter()
        .find(|(l, _)| *l == exact)
        .or_else(|| branches.iter().find(|(l, _)| one && l == "one"))
        .or_else(|| branches.iter().find(|(l, _)| l == "other"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_and_pluralizes() {
        assert_eq!(t("PAGES.HOME.MY_ADDONS_TAB_TITLE"), "My Addons");
        assert_eq!(tp("PAGES.MY_ADDONS.PAGE_CONTEXT_FOOTER.ADDONS_INSTALLED", &[("count", "1".into())]), "1 addon");
        assert_eq!(tp("PAGES.MY_ADDONS.PAGE_CONTEXT_FOOTER.ADDONS_INSTALLED", &[("count", "5".into())]), "5 addons");
        assert_eq!(t("NO.SUCH.KEY"), "NO.SUCH.KEY");
        for (code, _, _) in LANGUAGES {
            set_language(code);
            assert!(!t("PAGES.HOME.OPTIONS_TAB_TITLE").is_empty(), "{code}");
        }
        set_language("de");
        assert_ne!(t("PAGES.HOME.GET_ADDONS_TAB_TITLE"), "Get Addons");
        set_language("en");
    }
}
