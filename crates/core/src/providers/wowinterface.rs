// SPDX-License-Identifier: GPL-3.0-or-later
//! WowInterface (mmoui) provider, ported from `wow-interface-addon-provider.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::Deserialize;

use super::{bbcode_to_text, AddonProvider, Capabilities, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::Net;
use crate::toc::toc_for_folder;

const API_URL: &str = "https://api.mmoui.com/v4/game/WOW";
const API_URL_FALLBACK: &str = "https://api.mmoui.com/v3/game/WOW";
const ADDON_URL: &str = "https://www.wowinterface.com/downloads/info";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    #[serde(default)]
    pub thumb_url: String,
    #[serde(default)]
    pub image_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsV4 {
    pub id: u64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub last_update: i64,
    #[serde(default)]
    pub download_uri: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub change_log: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
struct DetailsV3 {
    UID: String,
    #[serde(default)]
    UIVersion: String,
    #[serde(default)]
    UIDate: i64,
    #[serde(default)]
    UIDownload: String,
    #[serde(default)]
    UIName: String,
    #[serde(default)]
    UIAuthorName: String,
    #[serde(default)]
    UIDescription: String,
    #[serde(default)]
    UIChangeLog: String,
    #[serde(default)]
    UIHitCount: String,
}

impl From<DetailsV3> for DetailsV4 {
    fn from(d: DetailsV3) -> Self {
        DetailsV4 {
            id: d.UID.parse().unwrap_or(0),
            version: d.UIVersion,
            last_update: d.UIDate,
            download_uri: d.UIDownload,
            title: d.UIName,
            author: d.UIAuthorName,
            description: d.UIDescription,
            change_log: d.UIChangeLog,
            downloads: d.UIHitCount.parse().unwrap_or(0),
            images: vec![],
        }
    }
}

pub struct WowInterfaceProvider {
    net: Arc<Net>,
}

impl WowInterfaceProvider {
    pub fn new(net: Arc<Net>) -> Self {
        Self { net }
    }

    async fn details_many(&self, ids: &[String]) -> Vec<DetailsV4> {
        if ids.is_empty() {
            return vec![];
        }
        let mut responses: Vec<DetailsV4> = Vec::new();
        let fallback_ids: Vec<String> =
            match self.net.get_json::<Vec<DetailsV4>>(&format!("{API_URL}/filedetails/{}.json", ids.join(",")), &[]).await {
                Ok(r) => {
                    responses = r;
                    ids.iter().filter(|id| !responses.iter().any(|r| &r.id.to_string() == *id)).cloned().collect()
                }
                Err(e) => {
                    tracing::warn!("[wowi] addon details failed, using fallback: {e:#}");
                    ids.to_vec()
                }
            };
        if !fallback_ids.is_empty() {
            let url = format!("{API_URL_FALLBACK}/filedetails/{}.json", fallback_ids.join(","));
            match self.net.get_json::<Vec<DetailsV3>>(&url, &[]).await {
                Ok(v3) => responses.extend(v3.into_iter().map(DetailsV4::from)),
                Err(e) => tracing::warn!("[wowi] fallback failed: {e:#}"),
            }
        }
        responses
    }

    async fn details(&self, id: &str) -> Option<DetailsV4> {
        if id.is_empty() {
            return None;
        }
        self.details_many(&[id.to_string()]).await.into_iter().next()
    }

    fn released(d: &DetailsV4) -> Option<chrono::DateTime<Utc>> {
        Utc.timestamp_millis_opt(d.last_update).single()
    }

    fn to_search_result(&self, d: &DetailsV4) -> AddonSearchResult {
        AddonSearchResult {
            author: d.author.clone(),
            external_id: d.id.to_string(),
            name: d.title.clone(),
            thumbnail_url: d.images.first().map(|i| i.thumb_url.clone()).unwrap_or_default(),
            external_url: format!("{ADDON_URL}{}", d.id),
            provider_name: PROVIDER_WOWINTERFACE.into(),
            download_count: Some(d.downloads),
            summary: Some(bbcode_to_text(&d.description)),
            screenshot_urls: d.images.iter().map(|i| i.image_url.clone()).collect(),
            files: vec![AddonSearchResultFile {
                external_id: None,
                channel_type: AddonChannelType::Stable,
                version: d.version.clone(),
                folders: vec![],
                game_version: String::new(),
                download_url: d.download_uri.clone(),
                release_date: Self::released(d),
                dependencies: vec![],
                changelog: Some(bbcode_to_text(&d.change_log)),
                title: None,
                authors: None,
            }],
            ..Default::default()
        }
    }

    fn to_addon(&self, d: &DetailsV4, inst: &WowInstallation, channel: AddonChannelType, folder: &AddonFolder) -> Addon {
        let toc = toc_for_folder(folder, inst.client_type);
        Addon {
            id: uuid::Uuid::new_v4().to_string(),
            name: d.title.clone(),
            author: Some(d.author.clone()),
            channel_type: channel,
            client_type: Some(inst.client_type),
            download_url: Some(d.download_uri.clone()),
            external_id: Some(d.id.to_string()),
            external_url: Some(format!("{ADDON_URL}{}", d.id)),
            game_version: toc.as_ref().map(|t| t.interface.iter().map(|i| game_version(i)).collect()).unwrap_or_default(),
            installed_at: Some(Utc::now()),
            installed_folders: Some(folder.name.clone()),
            installed_folder_list: vec![folder.name.clone()],
            installed_version: toc.and_then(|t| t.version),
            latest_version: Some(d.version.clone()),
            provider_name: Some(PROVIDER_WOWINTERFACE.into()),
            thumbnail_url: d.images.first().map(|i| i.thumb_url.clone()),
            summary: Some(bbcode_to_text(&d.description)),
            screenshot_urls: d.images.iter().map(|i| i.image_url.clone()).collect(),
            download_count: Some(d.downloads),
            released_at: Self::released(d),
            latest_changelog: Some(bbcode_to_text(&d.change_log)),
            external_channel: Some("Stable".into()),
            installation_id: Some(inst.id.clone()),
            ..Default::default()
        }
    }

    /// `https://www.wowinterface.com/downloads/download25538-Aardvark` or `.../info25538-...`.
    pub fn addon_id_from_url(url: &url::Url) -> Option<String> {
        let re = regex::Regex::new(r"(?i)/(?:download|info)(\d+)").unwrap();
        re.captures(url.path()).map(|c| c[1].to_string())
    }
}

#[async_trait]
impl AddonProvider for WowInterfaceProvider {
    fn name(&self) -> &'static str {
        PROVIDER_WOWINTERFACE
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { force_ignore: false, allow_reinstall: true, allow_edit: true, ..Default::default() }
    }

    async fn scan(&self, inst: &WowInstallation, channel: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        let mut ids: Vec<String> = Vec::new();
        for f in folders.iter() {
            for t in &f.tocs {
                if let Some(id) = &t.wow_interface_id {
                    if t.load_on_demand.as_deref() != Some("1") && !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let details = self.details_many(&ids).await;
        for folder in folders.iter_mut() {
            if !folder.tocs.iter().any(|t| t.wow_interface_id.is_some() && t.load_on_demand.as_deref() != Some("1")) {
                continue;
            }
            let Some(toc) = toc_for_folder(folder, inst.client_type) else { continue };
            let Some(wid) = toc.wow_interface_id else { continue };
            if let Some(d) = details.iter().find(|d| d.id.to_string() == wid) {
                folder.matching_addon = Some(self.to_addon(d, inst, channel, folder));
            }
        }
        Ok(())
    }

    async fn get_all(&self, _inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        let (valid, invalid): (Vec<String>, Vec<String>) = ids.iter().cloned().partition(|i| self.is_valid_addon_id(i));
        let mut errors: Vec<ProviderError> =
            invalid.into_iter().map(|i| ProviderError::msg(format!("invalid addon id found: {i}"))).collect();
        let results: Vec<AddonSearchResult> = self
            .details_many(&valid)
            .await
            .iter()
            .map(|d| self.to_search_result(d))
            .filter(|r| valid.contains(&r.external_id))
            .collect();
        for id in &valid {
            if !results.iter().any(|r| &r.external_id == id) {
                errors.push(ProviderError::removed(id.clone()));
            }
        }
        Ok(GetAllResult { search_results: results, errors })
    }

    async fn search_by_url(&self, url: &url::Url, _inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let id = Self::addon_id_from_url(url).ok_or_else(|| anyhow::anyhow!("Unhandled URL: {url}"))?;
        let d = self.details(&id).await.ok_or_else(|| anyhow::anyhow!("Bad addon api response {url}"))?;
        Ok(Some(self.to_search_result(&d)))
    }

    async fn get_by_id(&self, id: &str, _inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        Ok(self.details(id).await.map(|d| self.to_search_result(&d)))
    }

    fn is_valid_addon_uri(&self, url: &url::Url) -> bool {
        url.host_str().is_some_and(|h| h.ends_with("wowinterface.com"))
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        !id.is_empty() && id.chars().next().is_some_and(|c| c.is_ascii_digit())
    }

    async fn get_changelog(&self, _inst: &WowInstallation, id: &str, _release: &str) -> anyhow::Result<String> {
        Ok(self.details(id).await.map(|d| bbcode_to_text(&d.change_log)).unwrap_or_default())
    }

    async fn get_description(&self, _inst: &WowInstallation, id: &str) -> anyhow::Result<String> {
        Ok(self.details(id).await.map(|d| bbcode_to_text(&d.description)).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_ids() {
        let u = url::Url::parse("https://www.wowinterface.com/downloads/download25538-Aardvark").unwrap();
        assert_eq!(WowInterfaceProvider::addon_id_from_url(&u).as_deref(), Some("25538"));
        let u = url::Url::parse("https://www.wowinterface.com/downloads/info5108-Clique.html").unwrap();
        assert_eq!(WowInterfaceProvider::addon_id_from_url(&u).as_deref(), Some("5108"));
    }
}
