// SPDX-License-Identifier: GPL-3.0-or-later
//! Addon providers, ported from wowup-lib-core `addon-provider.ts` and the provider classes.

use std::sync::Arc;

use async_trait::async_trait;

use crate::models::{
    Addon, AddonCategory, AddonChannelType, AddonFolder, AddonSearchResult, AddonWarningType, ProtocolSearchResult,
    WowInstallation,
};
use crate::net::Net;

pub mod curseforge;
pub mod github;
pub mod hub;
pub mod raiderio;
pub mod tukui;
pub mod wago;
pub mod wowinterface;
pub mod zip;

/// A provider-side failure for one addon, carrying the warning it should leave on the addon
/// (`GenericProviderError` / `SourceRemovedAddonError`).
#[derive(Debug, Clone)]
pub struct ProviderError {
    pub addon_id: Option<String>,
    pub warning: Option<AddonWarningType>,
    pub message: String,
}

impl ProviderError {
    pub fn removed(addon_id: impl Into<String>) -> Self {
        let id = addon_id.into();
        Self {
            message: format!("addon not found on provider: {id}"),
            addon_id: Some(id),
            warning: Some(AddonWarningType::MissingOnProvider),
        }
    }

    pub fn msg(message: impl Into<String>) -> Self {
        Self { addon_id: None, warning: None, message: message.into() }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Default)]
pub struct GetAllResult {
    pub search_results: Vec<AddonSearchResult>,
    pub errors: Vec<ProviderError>,
}

#[derive(Debug, Default, Clone)]
pub struct DownloadAuth {
    pub headers: Vec<(String, String)>,
    pub query_params: Vec<(String, String)>,
}

/// Static capability flags.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub force_ignore: bool,
    pub allow_reinstall: bool,
    pub allow_channel_change: bool,
    pub allow_edit: bool,
    pub allow_view_at_source: bool,
    pub allow_rescan: bool,
    pub can_show_changelog: bool,
    pub can_batch_fetch: bool,
    pub auth_required: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            force_ignore: true,
            allow_reinstall: false,
            allow_channel_change: false,
            allow_edit: false,
            allow_view_at_source: true,
            allow_rescan: true,
            can_show_changelog: true,
            can_batch_fetch: false,
            auth_required: false,
        }
    }
}

#[async_trait]
pub trait AddonProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// Shown next to the provider in settings.
    fn note(&self) -> &'static str {
        ""
    }

    /// Whether the provider is usable at all (e.g. has its API key).
    fn is_configured(&self) -> bool {
        true
    }

    async fn scan(
        &self,
        _installation: &WowInstallation,
        _channel: AddonChannelType,
        _folders: &mut [AddonFolder],
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_all(&self, _installation: &WowInstallation, _ids: &[String]) -> anyhow::Result<GetAllResult> {
        Ok(GetAllResult::default())
    }

    async fn get_featured(
        &self,
        _installation: &WowInstallation,
        _channel: AddonChannelType,
    ) -> anyhow::Result<Vec<AddonSearchResult>> {
        Ok(vec![])
    }

    async fn search(
        &self,
        _query: &str,
        _installation: &WowInstallation,
        _channel: AddonChannelType,
    ) -> anyhow::Result<Vec<AddonSearchResult>> {
        Ok(vec![])
    }

    async fn search_by_url(
        &self,
        _url: &url::Url,
        _installation: &WowInstallation,
    ) -> anyhow::Result<Option<AddonSearchResult>> {
        Ok(None)
    }

    async fn search_protocol(&self, _protocol: &str) -> anyhow::Result<Option<ProtocolSearchResult>> {
        Ok(None)
    }

    async fn get_category(
        &self,
        _category: AddonCategory,
        _installation: &WowInstallation,
    ) -> anyhow::Result<Vec<AddonSearchResult>> {
        Ok(vec![])
    }

    async fn get_by_id(&self, _id: &str, _installation: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        Ok(None)
    }

    fn is_valid_addon_uri(&self, _url: &url::Url) -> bool {
        false
    }

    fn is_valid_addon_id(&self, _id: &str) -> bool {
        false
    }

    fn is_valid_protocol(&self, _protocol: &str) -> bool {
        false
    }

    fn should_migrate(&self, _addon: &Addon) -> bool {
        false
    }

    async fn get_changelog(
        &self,
        _installation: &WowInstallation,
        _external_id: &str,
        _release_id: &str,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }

    async fn get_description(
        &self,
        _installation: &WowInstallation,
        _external_id: &str,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }

    async fn download_auth(&self) -> Option<DownloadAuth> {
        None
    }
}

pub type ProviderRef = Arc<dyn AddonProvider>;

/// Settings the providers read at construction.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub hub_api_url: String,
    pub website_url: String,
    pub curseforge_api_key: Option<String>,
    pub wago_api_key: Option<String>,
    pub github_token: Option<String>,
}

impl ProviderConfig {
    pub fn from_prefs(prefs: &crate::preferences::Preferences) -> Self {
        Self {
            hub_api_url: std::env::var("WOWUP_HUB_URL").unwrap_or_else(|_| "https://hub.wowup.io".into()),
            website_url: "https://wowup.io".into(),
            curseforge_api_key: prefs.curseforge_api_key.clone().filter(|k| !k.is_empty()),
            wago_api_key: prefs.wago_api_key.clone().filter(|k| !k.is_empty()),
            github_token: prefs.github_token.clone().filter(|k| !k.is_empty()),
        }
    }
}

/// All providers in WowUp's registration order, which is also scan priority (`loadProviders`).
pub fn all_providers(net: Arc<Net>, cfg: &ProviderConfig) -> Vec<ProviderRef> {
    vec![
        Arc::new(zip::ZipProvider::new(net.clone())),
        Arc::new(raiderio::RaiderIoProvider),
        Arc::new(raiderio::WowUpCompanionProvider),
        Arc::new(hub::HubProvider::new(net.clone(), &cfg.hub_api_url, &cfg.website_url)),
        Arc::new(wago::WagoProvider::new(net.clone(), cfg.wago_api_key.clone())),
        Arc::new(curseforge::CurseForgeProvider::new(net.clone(), cfg.curseforge_api_key.clone())),
        Arc::new(tukui::TukUiProvider::new(net.clone())),
        Arc::new(wowinterface::WowInterfaceProvider::new(net.clone())),
        Arc::new(github::GitHubProvider::new(net, cfg.github_token.clone())),
    ]
}

/// Sørensen–Dice coefficient over character bigrams, as in the `string-similarity` package.
pub fn string_similarity(a: &str, b: &str) -> f64 {
    let a: String = a.chars().filter(|c| !c.is_whitespace()).collect();
    let b: String = b.chars().filter(|c| !c.is_whitespace()).collect();
    if a == b {
        return 1.0;
    }
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if ac.len() < 2 || bc.len() < 2 {
        return 0.0;
    }
    let mut first: std::collections::HashMap<(char, char), usize> = std::collections::HashMap::new();
    for w in ac.windows(2) {
        *first.entry((w[0], w[1])).or_default() += 1;
    }
    let mut intersection = 0;
    for w in bc.windows(2) {
        if let Some(n) = first.get_mut(&(w[0], w[1])) {
            if *n > 0 {
                *n -= 1;
                intersection += 1;
            }
        }
    }
    (2.0 * intersection as f64) / ((ac.len() + bc.len() - 2) as f64)
}

/// Strip HTML-ish bbcode to readable text (`convertBbcode`, simplified for a terminal).
pub fn bbcode_to_text(s: &str) -> String {
    let url = regex::Regex::new(r"(?i)\[url=([^\]]+)\]([^\[]*)\[/url\]").unwrap();
    let s = url.replace_all(s, "$2 ($1)");
    let s = regex::Regex::new(r"(?i)\[/?url\]").unwrap().replace_all(&s, "");
    let re = regex::Regex::new(
        r"(?i)\[/?(b|i|u|s|size|color|font|center|left|right|indent|quote|code|img|list|highlight|email)(=[^\]]*)?\]",
    )
    .unwrap();
    let s = re.replace_all(&s, "");
    s.replace("[*]", "• ").replace("\r\n", "\n").trim().to_string()
}

/// Strip HTML tags to plain text for terminal display.
pub fn html_to_text(s: &str) -> String {
    let s = regex::Regex::new(r"(?i)<br\s*/?>|</p>|</li>|</h\d>").unwrap().replace_all(s, "\n");
    let s = regex::Regex::new(r"(?i)<li[^>]*>").unwrap().replace_all(&s, "• ");
    let s = regex::Regex::new(r"<[^>]+>").unwrap().replace_all(&s, "");
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

pub fn parse_date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity() {
        assert_eq!(string_similarity("elvui", "elvui"), 1.0);
        assert!(string_similarity("elvui", "elvui shadow") > 0.5);
        assert!(string_similarity("tukui", "details") < 0.3);
    }

    #[test]
    fn bbcode() {
        assert_eq!(bbcode_to_text("[b]Hi[/b] [url=http://x]there[/url]"), "Hi there (http://x)");
    }
}
