// SPDX-License-Identifier: GPL-3.0-or-later
//! CurseForge provider (CF Core API v1), ported from `curse-addon-provider.ts`.
//!
//! WowUp's CurseForge build is ad supported and injects a private API key at build time. This port
//! has no ads and ships no key: the provider only works with a key the user supplies in settings
//! (keys are issued at https://console.curseforge.com).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{AddonProvider, Capabilities, DownloadAuth, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::{enc, Net};
use crate::toc::toc_for_folder;

/// CF Core API base; `WOWUP_CURSEFORGE_URL` overrides it (tests point it at a mock server).
fn api() -> String {
    std::env::var("WOWUP_CURSEFORGE_URL").unwrap_or_else(|_| "https://api.curseforge.com/v1".into())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SortableGameVersion {
    #[serde(default)]
    game_version_type_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Module {
    name: String,
    #[serde(default)]
    fingerprint: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dependency {
    mod_id: i64,
    relation_type: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    id: i64,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    file_date: Option<DateTime<Utc>>,
    #[serde(default)]
    release_type: i32,
    #[serde(default)]
    expose_as_alternative: Option<bool>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    sortable_game_versions: Vec<SortableGameVersion>,
    #[serde(default)]
    modules: Vec<Module>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Deserialize)]
struct Url {
    #[serde(default, rename = "websiteUrl")]
    website_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    #[serde(default)]
    thumbnail_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Shot {
    #[serde(default)]
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mod {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    download_count: f64,
    #[serde(default)]
    links: Option<Url>,
    #[serde(default)]
    logo: Option<Logo>,
    #[serde(default)]
    screenshots: Vec<Shot>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    latest_files: Vec<CfFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct FingerprintMatch {
    id: i64,
    file: CfFile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintResult {
    #[serde(default)]
    exact_matches: Vec<FingerprintMatch>,
    #[serde(default)]
    partial_matches: Vec<FingerprintMatch>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Featured {
    #[serde(default)]
    popular: Vec<Mod>,
    #[serde(default)]
    recently_updated: Vec<Mod>,
}

/// (gameVersionTypeId, client types) from `GAME_TYPE_LISTS`.
const GAME_TYPES: &[(i64, &[WowClientType])] = {
    use WowClientType::*;
    &[
        (67408, &[ClassicEra, ClassicEraPtr]),
        (73713, &[]),
        (517, &[Retail, RetailPtr, Beta, RetailXPtr]),
        (73246, &[Anniversary]),
        (77522, &[]),
        (79434, &[Classic, ClassicPtr]),
        (81212, &[]),
        (88568, &[Forever, ClassicBeta]),
    ]
};

fn game_version_type_id(c: WowClientType) -> Option<i64> {
    GAME_TYPES.iter().find(|(_, cs)| cs.contains(&c)).map(|(id, _)| *id)
}

fn channel_for(release_type: i32) -> AddonChannelType {
    match release_type {
        2 => AddonChannelType::Beta,
        3 => AddonChannelType::Alpha,
        _ => AddonChannelType::Stable,
    }
}

fn dep_type(relation: i32) -> AddonDependencyType {
    match relation {
        1 => AddonDependencyType::Embedded,
        2 => AddonDependencyType::Optional,
        3 => AddonDependencyType::Required,
        _ => AddonDependencyType::Other,
    }
}

fn category_id(c: AddonCategory) -> Option<i64> {
    use AddonCategory::*;
    Some(match c {
        Achievements => 1067,
        ActionBars => 1018,
        AuctionEconomy => 1002,
        BagsInventory => 1009,
        BossEncounters => 1014,
        BuffsDebuffs => 1005,
        ChatCommunication => 1001,
        Class => 1020,
        Combat => 1019,
        Companions => 1085,
        DataExport => 1007,
        DevelopmentTools => 1031,
        Guild => 1008,
        Libraries => 1010,
        Mail => 1012,
        MapMinimap => 1011,
        Miscellaneous => 1017,
        Missions => 1469,
        Plugins => 1063,
        Professions => 1015,
        Pvp => 1004,
        QuestsLeveling => 1013,
        Roleplay => 1060,
        Tooltips => 1055,
        UnitFrames => 1016,
        _ => return None,
    })
}

pub struct CurseForgeProvider {
    net: Arc<Net>,
    key: Option<String>,
}

impl CurseForgeProvider {
    pub fn new(net: Arc<Net>, key: Option<String>) -> Self {
        Self { net, key }
    }

    fn headers(&self) -> anyhow::Result<Vec<(&'static str, String)>> {
        let key = self.key.clone().ok_or_else(|| anyhow::anyhow!("CurseForge API key not set (Settings → Providers)"))?;
        Ok(vec![("x-api-key", key)])
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let d: Data<T> = self.net.get_json_cached(&format!("{}{path}", api()), &self.headers()?, Duration::from_secs(30)).await?;
        Ok(d.data)
    }

    async fn post<T: serde::de::DeserializeOwned>(&self, path: &str, body: serde_json::Value) -> anyhow::Result<T> {
        let d: Data<T> = self.net.post_json(&format!("{}{path}", api()), &body, &self.headers()?).await?;
        Ok(d.data)
    }

    fn compatible(c: WowClientType, f: &CfFile) -> bool {
        let Some(t) = game_version_type_id(c) else { return false };
        f.sortable_game_versions.iter().any(|s| s.game_version_type_id == t)
    }

    fn latest_files(m: &Mod, c: WowClientType) -> Vec<&CfFile> {
        let mut v: Vec<&CfFile> =
            m.latest_files.iter().filter(|f| f.expose_as_alternative != Some(true) && Self::compatible(c, f)).collect();
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    }

    fn to_file(f: &CfFile) -> AddonSearchResultFile {
        AddonSearchResultFile {
            external_id: Some(f.id.to_string()),
            channel_type: channel_for(f.release_type),
            version: f.display_name.clone(),
            folders: f.modules.iter().map(|m| m.name.clone()).collect(),
            game_version: game_version(f.game_versions.first().map(String::as_str).unwrap_or("")),
            download_url: f.download_url.clone().unwrap_or_default(),
            release_date: f.file_date,
            dependencies: f
                .dependencies
                .iter()
                .map(|d| AddonDependency { external_addon_id: d.mod_id.to_string(), dep_type: dep_type(d.relation_type) })
                .collect(),
            changelog: None,
            title: None,
            authors: None,
        }
    }

    fn to_result(m: &Mod, files: &[&CfFile]) -> AddonSearchResult {
        let mut out: Vec<AddonSearchResultFile> = files.iter().map(|f| Self::to_file(f)).collect();
        out.sort_by_key(|f| f.channel_type as u8);
        out.reverse();
        AddonSearchResult {
            author: m
                .authors
                .iter()
                .map(|a| a.name.clone())
                .filter(|n| !n.to_lowercase().starts_with("_forgeuser"))
                .collect::<Vec<_>>()
                .join(", "),
            external_id: m.id.to_string(),
            name: m.name.clone(),
            thumbnail_url: m.logo.as_ref().map(|l| l.thumbnail_url.clone()).unwrap_or_default(),
            external_url: m.links.as_ref().map(|l| l.website_url.clone()).unwrap_or_default(),
            provider_name: PROVIDER_CURSEFORGE.into(),
            files: out,
            download_count: Some(m.download_count as u64),
            summary: Some(m.summary.clone()),
            screenshot_urls: m.screenshots.iter().map(|s| s.url.clone()).filter(|u| !u.is_empty()).collect(),
            ..Default::default()
        }
    }

    fn create_addon(inst: &WowInstallation, folder: &AddonFolder, file: &CfFile, m: &Mod) -> Option<Addon> {
        let ct = inst.client_type;
        let channel = channel_for(file.release_type);
        let latest = Self::latest_files(m, ct);
        let lv = latest.iter().find(|f| channel.accepts(channel_for(f.release_type))).copied();
        let toc = toc_for_folder(folder, ct)?;
        let folders: Vec<String> = file.modules.iter().map(|m| m.name.clone()).collect();
        Some(Addon {
            id: uuid::Uuid::new_v4().to_string(),
            author: Some(m.authors.iter().map(|a| a.name.clone()).collect::<Vec<_>>().join(", ")),
            name: if m.name.is_empty() { "unknown".into() } else { m.name.clone() },
            channel_type: channel,
            client_type: Some(ct),
            download_url: lv.and_then(|l| l.download_url.clone()).or_else(|| file.download_url.clone()),
            external_url: m.links.as_ref().map(|l| l.website_url.clone()),
            external_id: Some(m.id.to_string()),
            game_version: toc.interface.iter().map(|i| game_version(i)).collect(),
            installed_at: folder.modified_at,
            installed_folders: Some(folders.join(",")),
            installed_folder_list: folders,
            installed_version: Some(file.display_name.clone()),
            installed_external_release_id: Some(file.id.to_string()),
            latest_version: Some(lv.map(|l| l.display_name.clone()).unwrap_or_else(|| file.display_name.clone())),
            provider_name: Some(PROVIDER_CURSEFORGE.into()),
            thumbnail_url: m.logo.as_ref().map(|l| l.thumbnail_url.clone()),
            screenshot_urls: m.screenshots.iter().map(|s| s.url.clone()).collect(),
            download_count: Some(m.download_count as u64),
            summary: Some(m.summary.clone()),
            released_at: lv.and_then(|l| l.file_date).or(file.file_date),
            external_latest_release_id: Some(lv.map(|l| l.id).unwrap_or(file.id).to_string()),
            updated_at: folder.modified_at,
            external_channel: Some(format!("{channel:?}")),
            installation_id: Some(inst.id.clone()),
            warning_type: latest.is_empty().then_some(AddonWarningType::NoProviderFiles),
            ..Default::default()
        })
    }

    async fn mods(&self, ids: &[i64]) -> anyhow::Result<Vec<Mod>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        self.post("/mods", json!({ "modIds": ids })).await
    }

    async fn search_mods(&self, inst: &WowInstallation, query: &str) -> anyhow::Result<Vec<Mod>> {
        let t = game_version_type_id(inst.client_type).ok_or_else(|| anyhow::anyhow!("unsupported client"))?;
        self.get(&format!(
            "/mods/search?gameId=1&searchFilter={}&sortField=1&sortOrder=desc&index=0&gameVersionTypeId={t}",
            enc(query)
        ))
        .await
    }

    fn strip_linkouts(html: &str) -> String {
        let re = regex::Regex::new(r#"href=(["'])(/linkout[^"']*)["']"#).unwrap();
        re.replace_all(html, |c: &regex::Captures| {
            let full = format!("https://www.curseforge.com{}", &c[2]);
            let remote = url::Url::parse(&full)
                .ok()
                .and_then(|u| u.query_pairs().find(|(k, _)| k == "remoteUrl").map(|(_, v)| v.into_owned()))
                .unwrap_or(c[2].to_string());
            format!("href={q}{remote}{q}", q = &c[1])
        })
        .into_owned()
    }
}

#[async_trait]
impl AddonProvider for CurseForgeProvider {
    fn name(&self) -> &'static str {
        PROVIDER_CURSEFORGE
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            force_ignore: false,
            allow_channel_change: true,
            allow_reinstall: true,
            can_batch_fetch: true,
            allow_edit: true,
            auth_required: true,
            ..Default::default()
        }
    }

    fn note(&self) -> &'static str {
        "Requires your own CurseForge API key (console.curseforge.com)."
    }

    fn is_configured(&self) -> bool {
        self.key.is_some()
    }

    async fn scan(&self, inst: &WowInstallation, _c: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        if self.key.is_none() {
            return Ok(());
        }
        let fps: Vec<u32> = folders.iter().filter_map(|f| f.cf_scan_results.as_ref().map(|s| s.fingerprint_num)).collect();
        if fps.is_empty() {
            return Ok(());
        }
        let res: FingerprintResult = self.post("/fingerprints", json!({ "fingerprints": fps })).await?;
        let mut pairs: Vec<(usize, FingerprintMatch)> = Vec::new();
        for (i, f) in folders.iter().enumerate() {
            let Some(fp) = f.cf_scan_results.as_ref().map(|s| s.fingerprint_num) else { continue };
            let m = res
                .exact_matches
                .iter()
                .find(|m| Self::compatible(inst.client_type, &m.file) && m.file.modules.iter().any(|x| x.fingerprint == fp))
                .or_else(|| res.partial_matches.iter().find(|m| m.file.modules.iter().any(|x| x.fingerprint == fp)));
            if let Some(m) = m {
                pairs.push((i, m.clone()));
            }
        }
        let mod_ids: Vec<i64> = pairs.iter().map(|(_, m)| m.id).collect();
        let mods = self.mods(&mod_ids).await?;
        let mut children: Vec<usize> = Vec::new();
        for (i, m) in &pairs {
            let Some(md) = mods.iter().find(|x| x.id == m.id) else { continue };
            match Self::create_addon(inst, &folders[*i], &m.file, md) {
                Some(a) => folders[*i].matching_addon = Some(a),
                None => children.push(*i),
            }
        }
        for c in children {
            let name = folders[c].name.clone();
            let parent = pairs
                .iter()
                .find(|(i, m)| folders[*i].matching_addon.is_some() && m.file.modules.iter().any(|x| x.name == name))
                .and_then(|(i, _)| folders[*i].matching_addon.clone());
            folders[c].matching_addon = parent;
        }
        Ok(())
    }

    async fn get_all(&self, inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        let nums: Vec<i64> = ids.iter().filter_map(|i| i.parse().ok()).collect();
        let mods = self.mods(&nums).await?;
        let mut out = GetAllResult::default();
        for m in &mods {
            let files = Self::latest_files(m, inst.client_type);
            if !files.is_empty() {
                out.search_results.push(Self::to_result(m, &files));
            }
        }
        for id in ids {
            if !mods.iter().any(|m| &m.id.to_string() == id) {
                out.errors.push(ProviderError::removed(id.clone()));
            }
        }
        Ok(out)
    }

    async fn get_by_id(&self, id: &str, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let Ok(m) = self.get::<Mod>(&format!("/mods/{id}")).await else { return Ok(None) };
        let files = Self::latest_files(&m, inst.client_type);
        Ok((!files.is_empty()).then(|| Self::to_result(&m, &files)))
    }

    async fn search(&self, q: &str, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let mods = self.search_mods(inst, q).await?;
        Ok(mods
            .iter()
            .filter_map(|m| {
                let f = Self::latest_files(m, inst.client_type);
                (!f.is_empty()).then(|| Self::to_result(m, &f))
            })
            .collect())
    }

    async fn search_by_url(&self, url: &url::Url, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let re = regex::Regex::new(r"(?i)/addons/(.*?)(/|$)").unwrap();
        let Some(slug) = re.captures(url.path()).map(|c| c[1].to_string()) else { return Ok(None) };
        let word = slug.split('-').next().unwrap_or(&slug).to_string();
        let mods = self.search_mods(inst, &word).await?;
        let m = mods.iter().find(|m| m.slug == slug).ok_or_else(|| anyhow::anyhow!("no-search-results"))?;
        let files = Self::latest_files(m, inst.client_type);
        if files.is_empty() {
            anyhow::bail!("no-latest-search-result-files");
        }
        Ok(Some(Self::to_result(m, &files)))
    }

    fn is_valid_addon_uri(&self, url: &url::Url) -> bool {
        url.host_str().is_some_and(|h| h.ends_with("curseforge.com")) && url.path().starts_with("/wow/addons")
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        id.chars().next().is_some_and(|c| c.is_ascii_digit())
    }

    fn is_valid_protocol(&self, p: &str) -> bool {
        p.to_ascii_lowercase().starts_with("curseforge://")
    }

    fn should_migrate(&self, addon: &Addon) -> bool {
        addon.installed_external_release_id.as_deref().unwrap_or("").is_empty()
    }

    async fn search_protocol(&self, protocol: &str) -> anyhow::Result<Option<ProtocolSearchResult>> {
        let u = url::Url::parse(protocol)?;
        let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
        let addon_id: i64 = q.get("addonId").and_then(|v| v.parse().ok()).unwrap_or(0);
        let file_id: i64 = q.get("fileId").and_then(|v| v.parse().ok()).unwrap_or(0);
        if addon_id == 0 || file_id == 0 {
            anyhow::bail!("Invalid protocol data");
        }
        let m: Mod = self.get(&format!("/mods/{addon_id}")).await?;
        let f: CfFile = self.get(&format!("/mods/{addon_id}/files/{file_id}")).await?;
        let types: Vec<WowClientType> = GAME_TYPES
            .iter()
            .filter(|(id, _)| f.sortable_game_versions.iter().any(|s| s.game_version_type_id == *id))
            .flat_map(|(_, cs)| cs.iter().copied())
            .collect();
        Ok(Some(ProtocolSearchResult {
            protocol: protocol.into(),
            protocol_addon_id: Some(addon_id.to_string()),
            protocol_release_id: Some(file_id.to_string()),
            valid_client_types: types,
            search_result: Self::to_result(&m, &[&f]),
        }))
    }

    async fn get_featured(&self, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let t = game_version_type_id(inst.client_type).ok_or_else(|| anyhow::anyhow!("unsupported client"))?;
        let f: Featured =
            self.post("/mods/featured", json!({ "gameId": 1, "gameVersionTypeId": t, "excludedModIds": [] })).await?;
        let mut list = f.popular.clone();
        list.extend(f.recently_updated.into_iter().filter(|r| !f.popular.iter().any(|p| p.id == r.id)));
        Ok(list
            .iter()
            .filter_map(|m| {
                let files = Self::latest_files(m, inst.client_type);
                (!files.is_empty()).then(|| Self::to_result(m, &files))
            })
            .collect())
    }

    async fn get_category(&self, c: AddonCategory, inst: &WowInstallation) -> anyhow::Result<Vec<AddonSearchResult>> {
        let t = game_version_type_id(inst.client_type).ok_or_else(|| anyhow::anyhow!("unsupported client"))?;
        let cat = match category_id(c) {
            Some(id) => format!("&categoryId={id}"),
            None => String::new(),
        };
        let mods: Vec<Mod> = self
            .get(&format!("/mods/search?gameId=1{cat}&pageSize=50&index=0&sortOrder=desc&gameVersionTypeId={t}"))
            .await?;
        Ok(mods
            .iter()
            .filter_map(|m| {
                let f = Self::latest_files(m, inst.client_type);
                (!f.is_empty()).then(|| Self::to_result(m, &f))
            })
            .collect())
    }

    async fn get_changelog(&self, _i: &WowInstallation, id: &str, release: &str) -> anyhow::Result<String> {
        Ok(self.get::<String>(&format!("/mods/{id}/files/{release}/changelog")).await.unwrap_or_default())
    }

    async fn get_description(&self, _i: &WowInstallation, id: &str) -> anyhow::Result<String> {
        Ok(self.get::<String>(&format!("/mods/{id}/description")).await.map(|d| Self::strip_linkouts(&d)).unwrap_or_default())
    }

    async fn download_auth(&self) -> Option<DownloadAuth> {
        self.key.as_ref().map(|k| DownloadAuth { headers: vec![("X-Api-Key".into(), k.clone())], query_params: vec![] })
    }
}
