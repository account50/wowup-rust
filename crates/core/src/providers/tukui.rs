// SPDX-License-Identifier: GPL-3.0-or-later
//! TukUI provider (ElvUI, Tukui and friends), ported from `tukui-addon-provider.ts`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use super::{parse_date, string_similarity, AddonProvider, Capabilities, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::Net;
use crate::toc::toc_for_folder;

const API_URL: &str = "https://api.tukui.org/v1/addons";
const DOWNLOAD_CT: u64 = 2_147_483_647;

#[derive(Debug, Clone, Deserialize)]
pub struct TukUiAddon {
    #[serde(deserialize_with = "id_string")]
    pub id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub changelog_url: String,
    #[serde(default)]
    pub patch: Vec<String>,
    #[serde(default)]
    pub last_update: String,
    #[serde(default)]
    pub web_url: String,
    #[serde(default)]
    pub small_desc: String,
    #[serde(default)]
    pub screenshot_url: String,
    #[serde(default)]
    pub directories: Vec<String>,
}

fn id_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    })
}

pub struct TukUiProvider {
    net: Arc<Net>,
}

impl TukUiProvider {
    pub fn new(net: Arc<Net>) -> Self {
        Self { net }
    }

    async fn all(&self, client: WowClientType) -> Vec<TukUiAddon> {
        if client == WowClientType::None {
            return vec![];
        }
        match self.net.get_json_cached::<Vec<TukUiAddon>>(API_URL, &[], Duration::from_secs(300)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[tukui] {e:#}");
                vec![]
            }
        }
    }

    fn patch_for(addon: &TukUiAddon, client: WowClientType) -> String {
        use WowClientType::*;
        let find = |p: &str| addon.patch.iter().find(|x| x.starts_with(p)).cloned().unwrap_or_default();
        match client {
            Classic | ClassicPtr => find("5."),
            ClassicEra | ClassicEraPtr => find("1.1"),
            Forever | ClassicBeta => find("1.6"),
            Anniversary => find("2."),
            _ => addon
                .patch
                .iter()
                .find(|p| !["1.1", "3.", "4.", "5.", "1.6"].iter().any(|t| p.starts_with(t)))
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn released(a: &TukUiAddon) -> Option<chrono::DateTime<Utc>> {
        parse_date(&a.last_update)
    }

    fn to_search_result(&self, a: &TukUiAddon, inst: &WowInstallation) -> AddonSearchResult {
        AddonSearchResult {
            author: a.author.clone(),
            external_id: a.id.clone(),
            name: a.name.clone(),
            thumbnail_url: a.screenshot_url.clone(),
            external_url: a.web_url.clone(),
            provider_name: PROVIDER_TUKUI.into(),
            download_count: Some(DOWNLOAD_CT),
            summary: Some(a.small_desc.clone()),
            files: vec![AddonSearchResultFile {
                external_id: None,
                channel_type: AddonChannelType::Stable,
                folders: a.directories.clone(),
                download_url: a.url.clone(),
                game_version: Self::patch_for(a, inst.client_type),
                version: a.version.clone(),
                release_date: Self::released(a),
                dependencies: vec![],
                changelog: None,
                title: None,
                authors: None,
            }],
            ..Default::default()
        }
    }

    fn search_names(addons: &[TukUiAddon], name: &str, allow_contain: bool) -> Vec<TukUiAddon> {
        let canon = name.to_lowercase();
        let mut scored: Vec<(f64, &TukUiAddon)> = addons
            .iter()
            .map(|a| (string_similarity(&canon, &a.name.to_lowercase()), a))
            .filter(|(s, _)| *s > 0.7)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut matches: Vec<TukUiAddon> = scored.into_iter().map(|(_, a)| a.clone()).collect();
        if allow_contain && matches.is_empty() {
            matches = addons.iter().filter(|a| a.name.to_lowercase().contains(&canon)).cloned().collect();
        }
        matches
    }

    fn create_addon(&self, toc: &Toc, folder: &AddonFolder, t: &TukUiAddon, inst: &WowInstallation, channel: AddonChannelType) -> Addon {
        let (folders_str, list) = match &toc.tuk_ui_project_folders {
            Some(f) => (f.clone(), f.split(',').map(|s| s.trim().to_string()).collect()),
            None => (folder.name.clone(), vec![folder.name.clone()]),
        };
        Addon {
            id: uuid::Uuid::new_v4().to_string(),
            channel_type: channel,
            client_type: Some(inst.client_type),
            name: t.name.clone(),
            author: Some(t.author.clone()),
            download_url: Some(t.url.clone()),
            external_id: Some(t.id.clone()),
            external_url: Some(t.web_url.clone()),
            game_version: toc.interface.iter().map(|i| game_version(i)).collect(),
            installed_at: folder.modified_at.or(Some(Utc::now())),
            installed_folders: Some(folders_str),
            installed_folder_list: list,
            installed_version: toc.version.clone(),
            latest_version: Some(t.version.clone()),
            provider_name: Some(PROVIDER_TUKUI.into()),
            thumbnail_url: Some(t.screenshot_url.clone()),
            updated_at: Some(Utc::now()),
            summary: Some(t.small_desc.clone()),
            download_count: Some(DOWNLOAD_CT),
            screenshot_urls: vec![t.screenshot_url.clone()],
            released_at: Self::released(t),
            external_channel: Some("Stable".into()),
            installation_id: Some(inst.id.clone()),
            ..Default::default()
        }
    }

    fn match_subfolders(folders: &mut [AddonFolder], inst: &WowInstallation) {
        let matched: Vec<Addon> = folders.iter().filter_map(|f| f.matching_addon.clone()).collect();
        for f in folders.iter_mut().filter(|f| f.matching_addon.is_none()) {
            if toc_for_folder(f, inst.client_type).is_none() {
                continue;
            }
            if let Some(parent) = matched.iter().find(|m| m.installed_folder_list.contains(&f.name)) {
                f.matching_addon = Some(parent.clone());
            }
        }
    }
}

#[async_trait]
impl AddonProvider for TukUiProvider {
    fn name(&self) -> &'static str {
        PROVIDER_TUKUI
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { force_ignore: false, allow_reinstall: true, allow_edit: true, ..Default::default() }
    }

    async fn scan(&self, inst: &WowInstallation, channel: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        let all = self.all(inst.client_type).await;
        if all.is_empty() {
            return Ok(());
        }
        // Only folders that a TukUI-tagged toc or a TukUI title could belong to are considered by id;
        // the name fallback runs over everything still unmatched, as in the original.
        for f in folders.iter_mut() {
            let Some(toc) = toc_for_folder(f, inst.client_type) else { continue };
            let Some(pid) = toc.tuk_ui_project_id.clone().filter(|p| !p.is_empty()) else { continue };
            if let Some(m) = all.iter().find(|a| a.id == pid) {
                f.matching_addon = Some(self.create_addon(&toc, f, m, inst, channel));
            }
        }
        Self::match_subfolders(folders, inst);
        for f in folders.iter_mut().filter(|f| f.matching_addon.is_none()) {
            let Some(toc) = toc_for_folder(f, inst.client_type) else { continue };
            let Some(title) = toc.title.clone() else { continue };
            if let Some(m) = Self::search_names(&all, &title, false).first() {
                f.matching_addon = Some(self.create_addon(&toc, f, m, inst, channel));
            }
        }
        Self::match_subfolders(folders, inst);
        Ok(())
    }

    async fn get_all(&self, inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        let (valid, invalid): (Vec<String>, Vec<String>) = ids.iter().cloned().partition(|i| self.is_valid_addon_id(i));
        let mut errors: Vec<ProviderError> =
            invalid.into_iter().map(|i| ProviderError::msg(format!("invalid addon id found: {i}"))).collect();
        let all = self.all(inst.client_type).await;
        let results: Vec<AddonSearchResult> =
            all.iter().filter(|a| valid.contains(&a.id)).map(|a| self.to_search_result(a, inst)).collect();
        for id in &valid {
            if !results.iter().any(|r| &r.external_id == id) {
                errors.push(ProviderError::removed(id.clone()));
            }
        }
        Ok(GetAllResult { search_results: results, errors })
    }

    async fn get_featured(&self, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        Ok(self.all(inst.client_type).await.iter().map(|a| self.to_search_result(a, inst)).collect())
    }

    async fn get_category(&self, _c: AddonCategory, inst: &WowInstallation) -> anyhow::Result<Vec<AddonSearchResult>> {
        self.get_featured(inst, AddonChannelType::Stable).await
    }

    async fn search(&self, q: &str, inst: &WowInstallation, _c: AddonChannelType) -> anyhow::Result<Vec<AddonSearchResult>> {
        let all = self.all(inst.client_type).await;
        Ok(Self::search_names(&all, q, true).iter().map(|a| self.to_search_result(a, inst)).collect())
    }

    async fn get_by_id(&self, id: &str, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        Ok(self.all(inst.client_type).await.iter().find(|a| a.id == id).map(|a| self.to_search_result(a, inst)))
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        id.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-')
    }

    async fn get_changelog(&self, inst: &WowInstallation, id: &str, _r: &str) -> anyhow::Result<String> {
        let all = self.all(inst.client_type).await;
        let Some(a) = all.iter().find(|a| a.id == id) else { return Ok(String::new()) };
        Ok(self.net.get_text(&a.changelog_url, &[]).await.unwrap_or_default())
    }

    async fn get_description(&self, inst: &WowInstallation, id: &str) -> anyhow::Result<String> {
        Ok(self.all(inst.client_type).await.iter().find(|a| a.id == id).map(|a| a.small_desc.clone()).unwrap_or_default())
    }
}
