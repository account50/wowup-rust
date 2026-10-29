// SPDX-License-Identifier: GPL-3.0-or-later
//! WowUp Hub provider (GitHub-hosted addons indexed by hub.wowup.io), ported from `wowup-addon-provider.ts`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{AddonProvider, Capabilities, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::{enc, Net};

#[derive(Debug, Clone, Deserialize)]
pub struct FundingLink {
    pub platform: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseFolder {
    pub folder_name: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameVersion {
    #[serde(default)]
    pub interface: String,
    #[serde(default)]
    pub title: String,
    pub game_type: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub authors: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Preview {
    pub url: String,
    pub preview_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub id: u64,
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub game_versions: Vec<GameVersion>,
    #[serde(default)]
    pub toc_title: Option<String>,
    #[serde(default)]
    pub toc_titles: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub download_url: String,
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default, rename = "addonFolders")]
    pub addon_folders: Option<Vec<ReleaseFolder>>,
    #[serde(default)]
    pub previews: Option<Vec<Preview>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubAddon {
    pub id: u64,
    #[serde(default)]
    pub repository_name: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub owner_name: Option<String>,
    #[serde(default)]
    pub owner_image_url: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub total_download_count: u64,
    #[serde(default)]
    pub matched_release: Option<Release>,
    #[serde(default)]
    pub releases: Option<Vec<Release>>,
    #[serde(default)]
    pub funding_links: Option<Vec<FundingLink>>,
}

#[derive(Deserialize)]
struct AddonsResponse {
    #[serde(default)]
    addons: Vec<HubAddon>,
    #[serde(default)]
    recent: Vec<HubAddon>,
}

#[derive(Deserialize)]
struct AddonResponse {
    addon: HubAddon,
}

#[derive(Deserialize)]
struct ReleaseResponse {
    release: Release,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintResponse {
    #[serde(default)]
    exact_matches: Vec<HubAddon>,
}

pub struct HubProvider {
    net: Arc<Net>,
    base: String,
    website: String,
}

fn group_for_game_type(s: &str) -> Option<WowClientGroup> {
    Some(match s {
        "retail" => WowClientGroup::Retail,
        "classic" => WowClientGroup::Classic,
        "burningCrusade" => WowClientGroup::BurningCrusade,
        "wotlk" => WowClientGroup::Wotlk,
        "cata" => WowClientGroup::Cata,
        "mists" => WowClientGroup::Mists,
        "forever" => WowClientGroup::Forever,
        _ => return None,
    })
}

impl HubProvider {
    pub fn new(net: Arc<Net>, base: &str, website: &str) -> Self {
        Self { net, base: base.trim_end_matches('/').into(), website: website.trim_end_matches('/').into() }
    }

    fn matching_version<'a>(release: &'a Release, game_type: &str) -> Option<&'a GameVersion> {
        release.game_versions.iter().find(|g| g.game_type == game_type)
    }

    fn file_for(release: &Release, gv: &GameVersion) -> AddonSearchResultFile {
        let version = if gv.version.is_empty() { release.tag_name.clone() } else { gv.version.clone() };
        AddonSearchResultFile {
            external_id: Some(release.id.to_string()),
            channel_type: if release.prerelease { AddonChannelType::Beta } else { AddonChannelType::Stable },
            version,
            folders: vec![],
            game_version: game_version(&gv.interface),
            download_url: release.download_url.clone(),
            release_date: release.published_at,
            dependencies: vec![],
            changelog: release.body.clone(),
            title: release
                .toc_titles
                .as_ref()
                .and_then(|t| t.get(&gv.game_type).cloned())
                .or_else(|| release.toc_title.clone())
                .or_else(|| Some(gv.title.clone()).filter(|t| !t.is_empty())),
            authors: Some(gv.authors.clone()).filter(|a| !a.is_empty()),
        }
    }

    fn screenshots<'a>(releases: impl Iterator<Item = &'a Release>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in releases {
            for p in r.previews.iter().flatten() {
                if p.preview_type == "image" && !out.contains(&p.url) {
                    out.push(p.url.clone());
                }
            }
        }
        out
    }

    fn build_result(&self, a: &HubAddon, files: Vec<AddonSearchResultFile>, releases: &[&Release]) -> Option<AddonSearchResult> {
        let first = files.first()?;
        Some(AddonSearchResult {
            author: first.authors.clone().or_else(|| a.owner_name.clone()).unwrap_or_default(),
            external_id: a.id.to_string(),
            external_url: format!("{}/addons/{}", self.website, a.id),
            name: first.title.clone().unwrap_or_else(|| a.repository_name.clone()),
            provider_name: PROVIDER_HUB.into(),
            thumbnail_url: a.image_url.clone().or_else(|| a.owner_image_url.clone()).unwrap_or_default(),
            download_count: Some(a.total_download_count),
            released_at: Some(Utc::now()),
            summary: a.description.clone(),
            funding_links: a
                .funding_links
                .iter()
                .flatten()
                .map(|f| AddonFundingLink { platform: f.platform.clone(), url: f.url.clone() })
                .collect(),
            screenshot_urls: Self::screenshots(releases.iter().copied()),
            files,
            ..Default::default()
        })
    }

    fn search_result(&self, a: &HubAddon, game_type: &str) -> Option<AddonSearchResult> {
        let releases: Vec<&Release> = a
            .releases
            .iter()
            .flatten()
            .filter(|r| Self::matching_version(r, game_type).is_some())
            .collect();
        let files: Vec<AddonSearchResultFile> = releases
            .iter()
            .filter_map(|r| Self::matching_version(r, game_type).map(|gv| Self::file_for(r, gv)))
            .collect();
        self.build_result(a, files, &releases)
    }

    fn to_addon(&self, inst: &WowInstallation, channel: AddonChannelType, a: &HubAddon) -> anyhow::Result<Addon> {
        let game_type = inst.client_type.game_type().as_str();
        let release = a.matched_release.as_ref().ok_or_else(|| anyhow::anyhow!("No matched release"))?;
        let folders: Vec<String> = release.addon_folders.iter().flatten().map(|f| f.folder_name.clone()).collect();
        let gv = Self::matching_version(release, game_type).or_else(|| release.game_versions.first());
        let name = release
            .toc_titles
            .as_ref()
            .and_then(|t| t.get(game_type).cloned())
            .filter(|t| !t.is_empty())
            .or_else(|| gv.map(|g| g.title.clone()).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| a.repository_name.clone());
        let version = gv.map(|g| g.version.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| release.tag_name.clone());
        let author = gv.map(|g| g.authors.clone()).filter(|v| !v.is_empty()).or_else(|| a.owner_name.clone());
        if name.is_empty() || version.is_empty() {
            anyhow::bail!("Invalid matching version data for {}", a.repository_name);
        }
        Ok(Addon {
            id: uuid::Uuid::new_v4().to_string(),
            author,
            name,
            channel_type: channel,
            client_type: Some(inst.client_type),
            download_url: Some(release.download_url.clone()),
            external_url: Some(format!("{}/addons/{}", self.website, a.id)),
            external_id: Some(a.id.to_string()),
            game_version: gv.map(|g| vec![game_version(&g.interface)]).unwrap_or_default(),
            installed_at: Some(Utc::now()),
            installed_folders: Some(folders.join(", ")),
            installed_folder_list: folders,
            installed_version: Some(version.clone()),
            installed_external_release_id: Some(release.id.to_string()),
            latest_version: Some(version),
            provider_name: Some(PROVIDER_HUB.into()),
            provider_source: a.source.clone(),
            thumbnail_url: a.image_url.clone(),
            funding_links: a
                .funding_links
                .iter()
                .flatten()
                .map(|f| AddonFundingLink { platform: f.platform.clone(), url: f.url.clone() })
                .collect(),
            released_at: release.published_at,
            external_channel: Some(format!("{channel:?}")),
            latest_changelog: release.body.clone(),
            external_latest_release_id: Some(release.id.to_string()),
            installation_id: Some(inst.id.clone()),
            screenshot_urls: Self::screenshots(std::iter::once(release)),
            ..Default::default()
        })
    }

    async fn by_fingerprints(&self, fingerprints: &[String]) -> anyhow::Result<Vec<HubAddon>> {
        if fingerprints.is_empty() {
            return Ok(vec![]);
        }
        let r: FingerprintResponse = self
            .net
            .post_json(&format!("{}/addons/fingerprint", self.base), &json!({ "fingerprints": fingerprints }), &[])
            .await?;
        Ok(r.exact_matches)
    }

    /// Fingerprint -> hub addon id, for addons installed from other providers (`getAddonIdsByFingerprints`).
    pub async fn addon_ids_by_fingerprints(&self, fingerprints: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for m in self.by_fingerprints(fingerprints).await? {
            for f in m.matched_release.iter().flat_map(|r| r.addon_folders.iter().flatten()) {
                if fingerprints.contains(&f.fingerprint) {
                    out.entry(f.fingerprint.clone()).or_insert_with(|| m.id.to_string());
                }
            }
        }
        Ok(out)
    }

    async fn addon_by_id(&self, id: &str) -> anyhow::Result<HubAddon> {
        let r: AddonResponse = self.net.get_json(&format!("{}/addons/{id}", self.base), &[]).await?;
        Ok(r.addon)
    }

    async fn release_by_id(&self, addon_id: &str, release_id: &str) -> anyhow::Result<Release> {
        let r: ReleaseResponse =
            self.net.get_json(&format!("{}/addons/{addon_id}/releases/{release_id}", self.base), &[]).await?;
        Ok(r.release)
    }
}

#[async_trait]
impl AddonProvider for HubProvider {
    fn name(&self) -> &'static str {
        PROVIDER_HUB
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            force_ignore: false,
            allow_reinstall: true,
            allow_channel_change: true,
            allow_edit: true,
            can_batch_fetch: true,
            ..Default::default()
        }
    }

    async fn scan(&self, inst: &WowInstallation, channel: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        let game_type = inst.client_type.game_type().as_str();
        let fps: Vec<String> = folders.iter().filter_map(|f| f.wowup_scan_results.as_ref().map(|s| s.fingerprint.clone())).collect();
        let matches = self.by_fingerprints(&fps).await?;
        for folder in folders.iter_mut() {
            let Some(fp) = folder.wowup_scan_results.as_ref().map(|s| s.fingerprint.clone()) else { continue };
            let candidates: Vec<&HubAddon> = matches
                .iter()
                .filter(|m| {
                    m.matched_release
                        .as_ref()
                        .and_then(|r| r.addon_folders.as_ref())
                        .is_some_and(|af| af.iter().any(|f| f.fingerprint == fp))
                })
                .collect();
            let pick = candidates
                .iter()
                .find(|m| m.matched_release.as_ref().is_some_and(|r| Self::matching_version(r, game_type).is_some()))
                .or_else(|| candidates.first());
            if let Some(m) = pick {
                match self.to_addon(inst, channel, m) {
                    Ok(a) => folder.matching_addon = Some(a),
                    Err(e) => tracing::warn!("[hub] {e:#}"),
                }
            }
        }
        Ok(())
    }

    async fn get_all(&self, inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        let game_type = inst.client_type.game_type().as_str();
        let id_nums: Vec<u64> = ids.iter().filter_map(|i| i.parse().ok()).collect();
        let r: AddonsResponse = self
            .net
            .post_json(&format!("{}/addons/batch/{game_type}", self.base), &json!({ "addonIds": id_nums }), &[])
            .await?;
        let results: Vec<AddonSearchResult> = r.addons.iter().filter_map(|a| self.search_result(a, game_type)).collect();
        let errors = ids
            .iter()
            .filter(|id| !results.iter().any(|r| &&r.external_id == id))
            .map(|id| ProviderError::removed(id.clone()))
            .collect();
        Ok(GetAllResult { search_results: results, errors })
    }

    async fn get_featured(&self, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let game_type = inst.client_type.game_type().as_str();
        let r: AddonsResponse = self
            .net
            .get_json_cached(
                &format!("{}/addons/featured/{game_type}?count=60&recent=30", self.base),
                &[],
                std::time::Duration::from_secs(30),
            )
            .await?;
        let mut list: Vec<&HubAddon> = r.addons.iter().collect();
        list.extend(r.recent.iter().filter(|x| !r.addons.iter().any(|p| p.id == x.id)));
        Ok(list.into_iter().filter_map(|a| self.search_result(a, game_type)).collect())
    }

    async fn search(&self, q: &str, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let game_type = inst.client_type.game_type().as_str();
        let r: AddonsResponse = self
            .net
            .get_json(&format!("{}/addons/search/{game_type}?query={}&limit=10", self.base, enc(q)), &[])
            .await?;
        Ok(r.addons.iter().filter_map(|a| self.search_result(a, game_type)).collect())
    }

    async fn get_category(&self, category: AddonCategory, inst: &WowInstallation) -> anyhow::Result<Vec<AddonSearchResult>> {
        let game_type = inst.client_type.game_type().as_str();
        let r: AddonsResponse = self
            .net
            .get_json(&format!("{}/addons/category/{}/{game_type}", self.base, category as u32), &[])
            .await?;
        Ok(r.addons.iter().filter_map(|a| self.search_result(a, game_type)).collect())
    }

    async fn get_by_id(&self, id: &str, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let a = self.addon_by_id(id).await?;
        Ok(self.search_result(&a, inst.client_type.game_type().as_str()))
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        id.parse::<u64>().is_ok_and(|n| n > 0)
    }

    fn is_valid_protocol(&self, p: &str) -> bool {
        p.to_ascii_lowercase().starts_with("wowup://")
    }

    fn should_migrate(&self, addon: &Addon) -> bool {
        addon.installed_external_release_id.as_deref().unwrap_or("").is_empty()
    }

    async fn search_protocol(&self, protocol: &str) -> anyhow::Result<Option<ProtocolSearchResult>> {
        let url = url::Url::parse(protocol)?;
        let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        let (Some(addon_id), Some(release_id)) = (q.get("addonId"), q.get("releaseId")) else {
            anyhow::bail!("Invalid protocol data");
        };
        let addon = self.addon_by_id(addon_id).await?;
        let release = self.release_by_id(addon_id, release_id).await?;
        let files = release.game_versions.iter().map(|gv| Self::file_for(&release, gv)).collect();
        let groups: Vec<WowClientGroup> = release.game_versions.iter().filter_map(|g| group_for_game_type(&g.game_type)).collect();
        let sr = self.build_result(&addon, files, &[&release]).ok_or_else(|| anyhow::anyhow!("Addon search result not created"))?;
        let valid_client_types = WowClientType::ALL.into_iter().filter(|c| groups.contains(&c.group())).collect();
        Ok(Some(ProtocolSearchResult {
            protocol: protocol.into(),
            protocol_addon_id: Some(addon_id.clone()),
            protocol_release_id: Some(release_id.clone()),
            valid_client_types,
            search_result: sr,
        }))
    }

    async fn get_changelog(&self, _i: &WowInstallation, id: &str, release_id: &str) -> anyhow::Result<String> {
        Ok(self.release_by_id(id, release_id).await.ok().and_then(|r| r.body).unwrap_or_default())
    }

    async fn get_description(&self, _i: &WowInstallation, id: &str) -> anyhow::Result<String> {
        Ok(self.addon_by_id(id).await.ok().and_then(|a| a.description).unwrap_or_default())
    }
}
