// SPDX-License-Identifier: GPL-3.0-or-later
//! Wago Addons provider, ported from `wago-addon-provider.ts`.
//!
//! Upstream WowUp gets a Wago API token from an embedded ad page unless the user has a Wago access key.
//! This port removes the ad path entirely: the provider works only with the user's own key.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use super::{parse_date, AddonProvider, Capabilities, DownloadAuth, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::{enc, Net};
use crate::toc::toc_for_folder;

/// Wago external API base; `WOWUP_WAGO_URL` overrides it (tests point it at a mock server).
fn base() -> String {
    std::env::var("WOWUP_WAGO_URL").unwrap_or_else(|_| "https://addons.wago.io/api/external".into())
}

#[derive(Debug, Clone, Deserialize)]
struct ScanRelease {
    #[serde(default)]
    id: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    supported_patches: Vec<String>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModuleHash {
    #[serde(default)]
    hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScanAddon {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    website_url: String,
    #[serde(default)]
    authors: Option<Vec<String>>,
    #[serde(default)]
    matched_release: Option<ScanRelease>,
    #[serde(default)]
    modules: Option<BTreeMap<String, ModuleHash>>,
    #[serde(default)]
    recent_releases: BTreeMap<String, ScanRelease>,
}

#[derive(Deserialize)]
struct ScanResponse {
    #[serde(default)]
    addons: Vec<Option<ScanAddon>>,
}

#[derive(Deserialize)]
struct RecentsResponse {
    #[serde(default)]
    addons: BTreeMap<String, ScanAddon>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchRelease {
    #[serde(default)]
    download_link: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    changelog: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchItem {
    id: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    thumbnail_image: Option<String>,
    #[serde(default)]
    website_url: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    gallery: Vec<String>,
    #[serde(default)]
    releases: BTreeMap<String, SearchRelease>,
    #[serde(default)]
    recent_releases: Option<BTreeMap<String, SearchRelease>>,
    #[serde(default)]
    recent_release: Option<BTreeMap<String, SearchRelease>>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<SearchItem>,
}

fn game_version_param(c: WowClientType) -> &'static str {
    match c.group() {
        WowClientGroup::Retail => "retail",
        WowClientGroup::Classic => "classic",
        WowClientGroup::BurningCrusade => "bc",
        WowClientGroup::Wotlk => "wotlk",
        WowClientGroup::Cata => "cata",
        WowClientGroup::Mists => "mop",
        WowClientGroup::Forever => "forever",
    }
}

fn stability(c: AddonChannelType) -> &'static str {
    match c {
        AddonChannelType::Alpha => "alpha",
        AddonChannelType::Beta => "beta",
        AddonChannelType::Stable => "stable",
    }
}

fn channel_of(s: &str) -> AddonChannelType {
    match s {
        "alpha" => AddonChannelType::Alpha,
        "beta" => AddonChannelType::Beta,
        _ => AddonChannelType::Stable,
    }
}

pub struct WagoProvider {
    net: Arc<Net>,
    key: Option<String>,
}

impl WagoProvider {
    pub fn new(net: Arc<Net>, key: Option<String>) -> Self {
        Self { net, key: key.filter(|k| k.len() > 20) }
    }

    fn headers(&self) -> anyhow::Result<Vec<(&'static str, String)>> {
        let k = self.key.clone().ok_or_else(|| anyhow::anyhow!("Wago API key required (Settings → Providers)"))?;
        Ok(vec![("Authorization", format!("Bearer {k}"))])
    }

    fn to_addon(&self, inst: &WowInstallation, channel: AddonChannelType, a: &ScanAddon) -> anyhow::Result<Addon> {
        let mut releases: Vec<(&String, &ScanRelease)> = a.recent_releases.iter().collect();
        releases.sort_by(|x, y| y.1.created_at.cmp(&x.1.created_at));
        let (stab, valid) = releases
            .into_iter()
            .find(|(k, _)| channel.accepts(channel_of(k)))
            .ok_or_else(|| anyhow::anyhow!("no valid release for {}", a.name))?;
        let folders: Vec<String> = a
            .modules
            .iter()
            .flatten()
            .filter(|(_, m)| m.hash.is_some())
            .map(|(k, _)| k.clone())
            .collect();
        let mr = a.matched_release.as_ref();
        Ok(Addon {
            id: uuid::Uuid::new_v4().to_string(),
            author: Some(a.authors.clone().unwrap_or_default().join(", ")),
            name: a.name.clone(),
            channel_type: channel_of(stab),
            client_type: Some(inst.client_type),
            download_url: valid.link.clone(),
            external_url: Some(a.website_url.clone()),
            external_id: Some(a.id.clone()),
            game_version: mr.map(|m| m.supported_patches.clone()).unwrap_or_default(),
            installed_at: Some(Utc::now()),
            installed_folders: Some(folders.join(",")),
            installed_folder_list: folders,
            installed_version: mr.map(|m| m.label.clone()),
            installed_external_release_id: mr.map(|m| m.id.clone()),
            latest_version: Some(valid.label.clone()),
            provider_name: Some(PROVIDER_WAGO.into()),
            thumbnail_url: a.thumbnail.clone(),
            released_at: mr.and_then(|m| parse_date(&m.created_at)),
            external_channel: Some(format!("{:?}", channel_of(stab))),
            external_latest_release_id: Some(valid.id.clone()),
            installation_id: Some(inst.id.clone()),
            ..Default::default()
        })
    }

    fn file(stab: &str, link: String, label: &str, created: &str) -> AddonSearchResultFile {
        AddonSearchResultFile {
            external_id: None,
            channel_type: channel_of(stab),
            download_url: link,
            folders: vec![],
            game_version: String::new(),
            release_date: parse_date(created),
            version: label.into(),
            dependencies: vec![],
            changelog: None,
            title: None,
            authors: None,
        }
    }

    fn from_scan(a: &ScanAddon) -> AddonSearchResult {
        AddonSearchResult {
            author: a.authors.clone().unwrap_or_default().join(", "),
            external_id: a.id.clone(),
            external_url: a.website_url.clone(),
            name: a.name.clone(),
            provider_name: PROVIDER_WAGO.into(),
            thumbnail_url: a.thumbnail.clone().unwrap_or_default(),
            download_count: Some(0),
            released_at: Some(Utc::now()),
            files: a
                .recent_releases
                .iter()
                .map(|(k, r)| {
                    let mut f = Self::file(k, r.link.clone().unwrap_or_default(), &r.label, &r.created_at);
                    f.external_id = Some(r.id.clone());
                    f
                })
                .collect(),
            ..Default::default()
        }
    }

    fn from_search(a: &SearchItem) -> AddonSearchResult {
        let rels = a.recent_releases.as_ref().or(a.recent_release.as_ref()).unwrap_or(&a.releases);
        AddonSearchResult {
            author: a.authors.join(", "),
            external_id: a.id.clone(),
            external_url: a.website_url.clone(),
            name: a.display_name.clone(),
            provider_name: PROVIDER_WAGO.into(),
            thumbnail_url: a.thumbnail_image.clone().unwrap_or_default(),
            download_count: Some(a.download_count),
            summary: Some(a.summary.clone()),
            screenshot_urls: a.gallery.clone(),
            released_at: Some(Utc::now()),
            files: rels.iter().map(|(k, r)| Self::file(k, r.download_link.clone(), &r.label, &r.created_at)).collect(),
            ..Default::default()
        }
    }

    async fn details(&self, id: &str) -> anyhow::Result<SearchItem> {
        self.net.get_json_cached(&format!("{}/addons/{id}", base()), &self.headers()?, Duration::from_secs(60)).await
    }
}

#[async_trait]
impl AddonProvider for WagoProvider {
    fn name(&self) -> &'static str {
        PROVIDER_WAGO
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            force_ignore: false,
            allow_reinstall: true,
            allow_channel_change: true,
            allow_edit: true,
            auth_required: true,
            ..Default::default()
        }
    }

    fn note(&self) -> &'static str {
        "Requires your own Wago API key (addons.wago.io → account). Ad-supported keys are not used."
    }

    fn is_configured(&self) -> bool {
        self.key.is_some()
    }

    async fn scan(&self, inst: &WowInstallation, channel: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        if self.key.is_none() {
            return Ok(());
        }
        let mut req = Vec::new();
        for f in folders.iter() {
            let (Some(sr), Some(toc)) = (&f.wowup_scan_results, toc_for_folder(f, inst.client_type)) else { continue };
            let mut entry = json!({ "name": f.name, "hash": sr.fingerprint });
            if let Some(w) = toc.wago_addon_id {
                entry["wago"] = json!(w);
            }
            req.push(entry);
        }
        if req.is_empty() {
            return Ok(());
        }
        let res: ScanResponse = self
            .net
            .post_json(
                &format!("{}/addons/_match", base()),
                &json!({ "game_version": game_version_param(inst.client_type), "addons": req }),
                &self.headers()?,
            )
            .await?;
        let addons: Vec<ScanAddon> = res.addons.into_iter().flatten().collect();
        for f in folders.iter_mut() {
            let Some(fp) = f.wowup_scan_results.as_ref().map(|s| s.fingerprint.clone()) else { continue };
            let m = addons.iter().find(|a| a.modules.iter().flatten().any(|(_, m)| m.hash.as_deref() == Some(&fp)));
            if let Some(m) = m {
                match self.to_addon(inst, channel, m) {
                    Ok(a) => f.matching_addon = Some(a),
                    Err(e) => tracing::warn!("[wago] {e:#}"),
                }
            }
        }
        Ok(())
    }

    async fn get_all(&self, inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        let res: RecentsResponse = self
            .net
            .post_json(
                &format!("{}/addons/_recents", base()),
                &json!({ "game_version": game_version_param(inst.client_type), "addons": ids }),
                &self.headers()?,
            )
            .await?;
        let results: Vec<AddonSearchResult> = res.addons.values().map(Self::from_scan).collect();
        let errors = ids
            .iter()
            .filter(|id| !results.iter().any(|r| &&r.external_id == id))
            .map(|id| ProviderError::removed(id.clone()))
            .collect();
        Ok(GetAllResult { search_results: results, errors })
    }

    async fn search(&self, q: &str, inst: &WowInstallation, c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let url = format!(
            "{}/addons/_search?query={}&game_version={}&stability={}",
            base(),
            enc(q),
            game_version_param(inst.client_type),
            stability(c)
        );
        let r: SearchResponse = self.net.get_json_cached(&url, &self.headers()?, Duration::from_secs(60)).await?;
        Ok(r.data.iter().map(Self::from_search).collect())
    }

    async fn get_featured(&self, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let url = format!("{}/addons/popular?game_version={}", base(), game_version_param(inst.client_type));
        let r: SearchResponse = self.net.get_json_cached(&url, &self.headers()?, Duration::from_secs(60)).await?;
        Ok(r.data.iter().map(Self::from_search).collect())
    }

    async fn get_by_id(&self, id: &str, _inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        Ok(Some(Self::from_search(&self.details(id).await?)))
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        (8..=10).contains(&id.len())
    }

    async fn get_description(&self, _i: &WowInstallation, id: &str) -> anyhow::Result<String> {
        Ok(self.details(id).await.ok().and_then(|d| d.description).unwrap_or_default())
    }

    async fn get_changelog(&self, inst: &WowInstallation, id: &str, _r: &str) -> anyhow::Result<String> {
        let Ok(d) = self.details(id).await else { return Ok(String::new()) };
        let Some(rel) = d.recent_release.as_ref().or(d.recent_releases.as_ref()) else { return Ok(String::new()) };
        let pick = match inst.default_addon_channel_type {
            AddonChannelType::Alpha => rel.get("alpha").or(rel.get("stable")),
            AddonChannelType::Beta => rel.get("beta").or(rel.get("stable")),
            AddonChannelType::Stable => rel.get("stable"),
        };
        Ok(pick.and_then(|r| r.changelog.clone()).unwrap_or_default())
    }

    async fn download_auth(&self) -> Option<DownloadAuth> {
        self.key.as_ref().map(|k| DownloadAuth { headers: vec![], query_params: vec![("token".into(), k.clone())] })
    }
}
