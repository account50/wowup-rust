// SPDX-License-Identifier: GPL-3.0-or-later
//! Local pseudo-providers that group known multi-folder addons and keep them ignored:
//! `raiderio-provider.ts` and `wowup-companion-addon-provider.ts`.

use async_trait::async_trait;
use chrono::Utc;

use super::{AddonProvider, Capabilities};
use crate::models::*;
use crate::toc::toc_for_folder;

fn base_addon(inst: &WowInstallation, provider: &str, toc: &Toc) -> Addon {
    Addon {
        id: uuid::Uuid::new_v4().to_string(),
        channel_type: AddonChannelType::Stable,
        is_ignored: true,
        client_type: Some(inst.client_type),
        provider_name: Some(provider.into()),
        external_id: Some(provider.into()),
        game_version: toc.interface.iter().map(|i| game_version(i)).collect(),
        is_load_on_demand: toc.load_on_demand.as_deref() == Some("1"),
        summary: toc.notes.clone(),
        download_count: Some(0),
        external_channel: Some("Stable".into()),
        installation_id: Some(inst.id.clone()),
        download_url: Some(String::new()),
        ..Default::default()
    }
}

pub struct RaiderIoProvider;

#[async_trait]
impl AddonProvider for RaiderIoProvider {
    fn name(&self) -> &'static str {
        PROVIDER_RAIDERIO
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    async fn scan(&self, inst: &WowInstallation, _c: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        let Some(main_idx) = folders.iter().position(|f| {
            f.name == "RaiderIO"
                && f.tocs.iter().any(|t| {
                    t.website.as_deref() == Some("https://raider.io") && t.addon_provider.as_deref() == Some("raiderio-client")
                })
        }) else {
            return Ok(());
        };
        let Some(target) = toc_for_folder(&folders[main_idx], inst.client_type) else { return Ok(()) };
        let mut group: Vec<usize> = vec![main_idx];
        for (i, f) in folders.iter().enumerate() {
            if i != main_idx && f.tocs.iter().any(|t| t.dependency_list.iter().any(|d| d == "RaiderIO")) {
                group.push(i);
            }
        }
        let names: Vec<String> = group.iter().map(|i| folders[*i].name.clone()).collect();
        let now = Utc::now();
        for i in group {
            let Some(sub) = toc_for_folder(&folders[i], inst.client_type) else { continue };
            let mut a = base_addon(inst, PROVIDER_RAIDERIO, &sub);
            a.name = target.title.clone().unwrap_or_else(|| "unknown".into());
            a.author = sub.author.clone();
            a.external_url = Some("https://raider.io".into());
            a.installed_at = Some(now);
            a.updated_at = Some(now);
            a.released_at = Some(now);
            a.installed_folders = Some(names.join(","));
            a.installed_folder_list = names.clone();
            a.installed_version = sub.version.clone().or_else(|| target.version.clone());
            a.latest_version = sub.version.clone();
            a.thumbnail_url = Some("http://cdnassets.raider.io/images/fb_app_image.jpg?2019-11-18".into());
            folders[i].matching_addon = Some(a);
        }
        Ok(())
    }
}

pub struct WowUpCompanionProvider;

#[async_trait]
impl AddonProvider for WowUpCompanionProvider {
    fn name(&self) -> &'static str {
        PROVIDER_WOWUP_COMPANION
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    async fn scan(&self, inst: &WowInstallation, _c: AddonChannelType, folders: &mut [AddonFolder]) -> anyhow::Result<()> {
        let Some(f) = folders.iter_mut().find(|f| {
            f.name == "wowup_data_addon"
                && f.tocs.iter().any(|t| t.website.as_deref() == Some("https://wowup.io"))
                && f.tocs.iter().any(|t| t.addon_provider.as_deref() == Some("wowup-app"))
        }) else {
            return Ok(());
        };
        let Some(toc) = toc_for_folder(f, inst.client_type) else { return Ok(()) };
        let updated = crate::fsutil::latest_mtime(std::path::Path::new(&f.path));
        let mut a = base_addon(inst, PROVIDER_WOWUP_COMPANION, &toc);
        a.name = toc.title.clone().unwrap_or_else(|| "unknown".into());
        a.author = Some(toc.author.clone().unwrap_or_else(|| "unknown".into()));
        a.external_url = Some("https://wowup.io".into());
        a.installed_at = updated;
        a.updated_at = updated;
        a.released_at = updated;
        a.installed_folders = Some(f.name.clone());
        a.installed_folder_list = vec![f.name.clone()];
        a.installed_version = toc.version.clone();
        a.latest_version = toc.version.clone();
        a.thumbnail_url = Some("https://avatars.githubusercontent.com/u/74023737?s=400&v=4".into());
        f.matching_addon = Some(a);
        Ok(())
    }
}
