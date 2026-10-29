// SPDX-License-Identifier: GPL-3.0-or-later
//! The addon pipeline: scan, sync, install/update/remove, dependencies and per-addon settings.
//! Ported from `addon.service.ts`, `addon-install.service.ts`, `addon-ui.service.ts` and
//! `addon-fingerprint.service.ts`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use tokio::sync::Semaphore;

use crate::fingerprint::{scan_folders, HashesNeeded};
use crate::fsutil;
use crate::installations::Installations;
use crate::models::*;
use crate::net::Net;
use crate::preferences::Preferences;
use crate::providers::{all_providers, hub::HubProvider, ProviderConfig, ProviderError, ProviderRef};
use crate::storage::Storage;
use crate::toc::{read_folder_tocs, toc_for_folder};

/// Folders WowUp never treats as addons.
const IGNORED_FOLDER_NAMES: &[&str] = &["__MACOSX"];
/// Install queue concurrency (`mergeMap` limit upstream).
const INSTALL_CONCURRENCY: usize = 3;
const DOWNLOAD_RETRIES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum InstallState {
    Pending,
    Downloading,
    BackingUp,
    Installing,
    Complete,
    Retry,
    Error,
}

pub type ProgressFn = Arc<dyn Fn(&Addon, InstallState, u8) + Send + Sync>;

pub fn no_progress() -> ProgressFn {
    Arc::new(|_, _, _| {})
}

/// An error emitted while syncing, for display (`AddonSyncError`).
#[derive(Debug, Clone)]
pub struct SyncIssue {
    pub provider: String,
    pub installation: String,
    pub addon: Option<String>,
    pub message: String,
}

/// Required-dependency details for the detail view (`getDependencyDetails`).
#[derive(Debug, Clone)]
pub struct DependencyInfo {
    pub external_id: String,
    pub name: String,
    pub installed: bool,
}

pub struct AddonService {
    pub storage: Arc<Storage>,
    pub net: Arc<Net>,
    providers: RwLock<Vec<ProviderRef>>,
    hub: RwLock<Arc<HubProvider>>,
    prefs: RwLock<Preferences>,
    install_slots: Semaphore,
    pending_deps: Mutex<HashSet<String>>,
    active: Mutex<BTreeMap<String, (InstallState, u8)>>,
}

impl AddonService {
    pub fn new(storage: Arc<Storage>) -> Result<Arc<Self>> {
        let prefs = storage.preferences()?;
        let net = Arc::new(Net::with_options(prefs.disable_ipv6));
        let cfg = ProviderConfig::from_prefs(&prefs);
        let svc = Arc::new(Self {
            providers: RwLock::new(all_providers(net.clone(), &cfg)),
            hub: RwLock::new(Arc::new(HubProvider::new(net.clone(), &cfg.hub_api_url, &cfg.website_url))),
            storage,
            net,
            prefs: RwLock::new(prefs),
            install_slots: Semaphore::new(INSTALL_CONCURRENCY),
            pending_deps: Mutex::new(HashSet::new()),
            active: Mutex::new(BTreeMap::new()),
        });
        let _ = std::fs::create_dir_all(svc.downloads_dir());
        Ok(svc)
    }

    // ---- preferences / providers ---------------------------------------------------------------

    pub fn prefs(&self) -> Preferences {
        self.prefs.read().unwrap().clone()
    }

    /// Persist preferences and rebuild providers so keys/tokens take effect immediately.
    pub fn save_prefs(&self, prefs: Preferences) -> Result<()> {
        self.storage.save_preferences(&prefs)?;
        let cfg = ProviderConfig::from_prefs(&prefs);
        *self.providers.write().unwrap() = all_providers(self.net.clone(), &cfg);
        *self.hub.write().unwrap() = Arc::new(HubProvider::new(self.net.clone(), &cfg.hub_api_url, &cfg.website_url));
        *self.prefs.write().unwrap() = prefs;
        self.net.clear_cache();
        Ok(())
    }

    pub fn all_providers(&self) -> Vec<ProviderRef> {
        self.providers.read().unwrap().clone()
    }

    /// Enabled and usable providers, in scan priority order.
    pub fn enabled_providers(&self) -> Vec<ProviderRef> {
        let prefs = self.prefs.read().unwrap();
        self.all_providers().into_iter().filter(|p| prefs.is_provider_enabled(p.name()) && p.is_configured()).collect()
    }

    pub fn provider(&self, name: &str) -> Option<ProviderRef> {
        let name = if name == PROVIDER_HUB_LEGACY { PROVIDER_HUB } else { name };
        self.all_providers().into_iter().find(|p| p.name() == name)
    }

    pub fn is_force_ignore(&self, a: &Addon) -> bool {
        a.is_unknown() || self.provider(a.provider()).is_some_and(|p| p.capabilities().force_ignore)
    }

    pub fn can_reinstall(&self, a: &Addon) -> bool {
        a.warning_type.is_none() && !a.is_unknown() && self.provider(a.provider()).is_some_and(|p| p.capabilities().allow_reinstall)
    }

    pub fn can_change_channel(&self, a: &Addon) -> bool {
        !a.is_unknown() && self.provider(a.provider()).is_some_and(|p| p.capabilities().allow_channel_change)
    }

    fn installations(&self) -> Installations<'_> {
        Installations::new(&self.storage)
    }

    fn installation(&self, a: &Addon) -> Result<WowInstallation> {
        let id = a.installation_id.as_deref().ok_or_else(|| anyhow!("addon has no installation"))?;
        self.installations().get(id)?.ok_or_else(|| anyhow!("installation not found"))
    }

    fn downloads_dir(&self) -> PathBuf {
        self.storage.root().join("downloads")
    }

    /// Delete leftovers from interrupted downloads (`cleanupDownloads`, run at startup).
    pub fn cleanup_downloads(&self) {
        let _ = fsutil::remove_path(&self.downloads_dir());
        let _ = std::fs::create_dir_all(self.downloads_dir());
    }

    // ---- queries -------------------------------------------------------------------------------

    /// Stored addons for an installation, scanning first if none are stored (`getAddons`).
    pub async fn get_addons(&self, inst: &WowInstallation, rescan: bool) -> Result<Vec<Addon>> {
        let addons = self.storage.addons(&inst.id)?;
        if rescan || addons.is_empty() {
            return self.rescan(inst).await;
        }
        Ok(addons)
    }

    pub fn find(&self, inst_id: &str, addon_id: &str) -> Result<Option<Addon>> {
        Ok(self.storage.addons(inst_id)?.into_iter().find(|a| a.id == addon_id))
    }

    pub fn find_by_external(&self, inst_id: &str, provider: &str, external_id: &str) -> Result<Option<Addon>> {
        Ok(self
            .storage
            .addons(inst_id)?
            .into_iter()
            .find(|a| a.provider() == provider && a.external_id.as_deref() == Some(external_id)))
    }

    pub fn is_installed(&self, inst_id: &str, provider: &str, external_id: &str) -> bool {
        self.find_by_external(inst_id, provider, external_id).ok().flatten().is_some()
    }

    pub fn updates_available(&self, inst_id: &str) -> usize {
        self.storage.addons(inst_id).map(|v| v.iter().filter(|a| a.needs_update()).count()).unwrap_or(0)
    }

    pub fn install_status(&self, addon_id: &str) -> Option<(InstallState, u8)> {
        self.active.lock().unwrap().get(addon_id).copied()
    }

    fn save(&self, a: &Addon) -> Result<()> {
        let inst = a.installation_id.as_deref().ok_or_else(|| anyhow!("addon has no installation"))?;
        self.storage.upsert_addon(inst, a)
    }

    // ---- scan ----------------------------------------------------------------------------------

    /// Addon folders on disk: each child directory (or symlink in symlink mode) with at least one toc.
    pub fn list_addon_folders(&self, inst: &WowInstallation) -> Vec<AddonFolder> {
        let symlinks = self.prefs().use_symlink_mode;
        let Ok(entries) = std::fs::read_dir(inst.addons_path()) else { return vec![] };
        let mut out: Vec<AddonFolder> = entries
            .flatten()
            .filter(|e| {
                let Ok(ft) = e.file_type() else { return false };
                ft.is_dir() || (symlinks && ft.is_symlink() && e.path().is_dir())
            })
            .filter(|e| !IGNORED_FOLDER_NAMES.contains(&e.file_name().to_string_lossy().as_ref()))
            .filter_map(|e| {
                let path = e.path();
                let tocs = read_folder_tocs(&path);
                if tocs.is_empty() {
                    return None;
                }
                let has_git = std::fs::read_dir(&path)
                    .map(|rd| rd.flatten().any(|c| c.file_name().to_string_lossy().eq_ignore_ascii_case(".git") && c.path().is_dir()))
                    .unwrap_or(false);
                Some(AddonFolder {
                    name: e.file_name().to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                    ignore_reason: has_git.then_some(AddonIgnoreReason::GitRepo),
                    tocs,
                    matching_addon: None,
                    modified_at: fsutil::mtime(&path),
                    cf_scan_results: None,
                    wowup_scan_results: None,
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    fn insert_external_id(&self, ids: &mut Vec<AddonExternalId>, provider: &str, id: Option<&str>) {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        if provider == PROVIDER_RAIDERIO || provider == PROVIDER_WOWUP_COMPANION {
            return;
        }
        if ids.iter().any(|e| e.provider_name == provider && e.id == id) {
            return;
        }
        let Some(p) = self.provider(provider) else { return };
        if !p.is_valid_addon_id(id) {
            return;
        }
        ids.push(AddonExternalId { provider_name: provider.into(), id: id.into() });
    }

    /// Record every provider id found in the toc plus the addon's own (`setExternalIds`).
    fn set_external_ids(&self, a: &mut Addon, toc: &Toc) {
        let mut ids = a.external_ids.clone();
        self.insert_external_id(&mut ids, PROVIDER_WOWINTERFACE, toc.wow_interface_id.as_deref());
        self.insert_external_id(&mut ids, PROVIDER_TUKUI, toc.tuk_ui_project_id.as_deref());
        self.insert_external_id(&mut ids, PROVIDER_CURSEFORGE, toc.curse_project_id.as_deref());
        self.insert_external_id(&mut ids, PROVIDER_WAGO, toc.wago_addon_id.as_deref());
        let provider = a.provider().to_string();
        self.insert_external_id(&mut ids, &provider, a.external_id.as_deref());
        a.external_ids = ids;
    }

    fn unmatched_addon(&self, folder: &AddonFolder, inst: &WowInstallation, matched: &[String]) -> Addon {
        let toc = toc_for_folder(folder, inst.client_type).unwrap_or_default();
        let title_ok = toc.title.as_deref().is_some_and(|t| t.chars().any(|c| c.is_ascii_alphabetic()));
        Addon {
            id: uuid::Uuid::new_v4().to_string(),
            name: if title_ok { toc.title.clone().unwrap() } else { folder.name.clone() },
            thumbnail_url: Some(String::new()),
            latest_version: Some(toc.version.clone().unwrap_or_default()),
            installed_version: Some(toc.version.clone().unwrap_or_default()),
            client_type: Some(inst.client_type),
            external_id: Some(String::new()),
            game_version: toc.interface.iter().map(|i| game_version(i)).collect(),
            author: Some(toc.author.clone().unwrap_or_default()),
            download_url: Some(String::new()),
            external_url: Some(String::new()),
            provider_name: Some(PROVIDER_UNKNOWN.into()),
            channel_type: AddonChannelType::Stable,
            is_ignored: true,
            released_at: fsutil::latest_mtime(Path::new(&folder.path)),
            installed_at: folder.modified_at.or(Some(Utc::now())),
            installed_folders: Some(folder.name.clone()),
            installed_folder_list: vec![folder.name.clone()],
            summary: Some(toc.notes.clone().unwrap_or_default()),
            is_load_on_demand: toc.load_on_demand.as_deref() == Some("1"),
            external_channel: Some("Stable".into()),
            missing_dependencies: toc.dependency_list.iter().filter(|d| !matched.contains(d)).cloned().collect(),
            ignore_reason: folder.ignore_reason,
            installation_id: Some(inst.id.clone()),
            ..Default::default()
        }
    }

    /// Match the AddOns folder against every enabled provider (`scanAddons`).
    pub async fn scan_addons(&self, inst: &WowInstallation, current: Option<&[Addon]>) -> Result<Vec<Addon>> {
        let mut folders = self.list_addon_folders(inst);
        if folders.is_empty() {
            return Ok(vec![]);
        }
        let mut addon_list: Vec<Addon> = Vec::new();

        // Addons whose provider cannot rescan (GitHub, Zip) keep their folders and records as-is.
        if let Some(current) = current {
            for a in current {
                if self.provider(a.provider()).is_some_and(|p| !p.capabilities().allow_rescan) {
                    folders.retain(|f| !a.installed_folder_list.contains(&f.name));
                    addon_list.push(a.clone());
                }
            }
        }

        let providers = self.enabled_providers();
        let needed = HashesNeeded {
            wowup: true,
            curseforge: providers.iter().any(|p| p.name() == PROVIDER_CURSEFORGE),
        };
        let paths: Vec<PathBuf> = folders.iter().map(|f| PathBuf::from(&f.path)).collect();
        for (path, res) in scan_folders(paths, needed).await {
            if let Some(f) = folders.iter_mut().find(|f| Path::new(&f.path) == path) {
                f.wowup_scan_results = res.wowup;
                f.cf_scan_results = res.curseforge;
            }
        }

        for p in &providers {
            // Each provider only sees folders that no earlier provider claimed.
            let idx: Vec<usize> = folders
                .iter()
                .enumerate()
                .filter(|(_, f)| f.ignore_reason.is_none() && f.matching_addon.is_none() && !f.tocs.is_empty())
                .map(|(i, _)| i)
                .collect();
            if idx.is_empty() {
                break;
            }
            let mut subset: Vec<AddonFolder> = idx.iter().map(|i| folders[*i].clone()).collect();
            if let Err(e) = p.scan(inst, inst.default_addon_channel_type, &mut subset).await {
                tracing::warn!("[scan] {} failed: {e:#}", p.name());
                continue;
            }
            for (i, f) in idx.into_iter().zip(subset) {
                folders[i].matching_addon = f.matching_addon;
            }
        }

        let matched_names: Vec<String> =
            folders.iter().filter(|f| f.matching_addon.is_some()).map(|f| f.name.clone()).collect();
        for f in folders.iter_mut() {
            let Some(toc) = toc_for_folder(f, inst.client_type) else { continue };
            let name = f.name.clone();
            if let Some(a) = f.matching_addon.as_mut() {
                if !toc.file_name.starts_with(&name) {
                    a.warning_type = Some(AddonWarningType::TocNameMismatch);
                }
                self.set_external_ids(a, &toc);
            }
        }

        // One addon per provider+id; the record with the most external ids wins.
        let mut groups: BTreeMap<String, Vec<Addon>> = BTreeMap::new();
        for f in &folders {
            if let Some(a) = &f.matching_addon {
                groups.entry(format!("{}{}", a.provider(), a.external_id.clone().unwrap_or_default())).or_default().push(a.clone());
            }
        }
        for (_, mut g) in groups {
            g.sort_by(|a, b| b.external_ids.len().cmp(&a.external_ids.len()));
            addon_list.push(g.remove(0));
        }

        for f in folders.iter().filter(|f| f.matching_addon.is_none()) {
            let toc = toc_for_folder(f, inst.client_type);
            let is_child = toc.as_ref().is_some_and(|t| {
                t.load_on_demand.as_deref() == Some("1") && t.dependency_list.iter().all(|d| matched_names.contains(d))
            });
            if !is_child {
                addon_list.push(self.unmatched_addon(f, inst, &matched_names));
            }
        }

        for a in addon_list.iter_mut() {
            a.latest_changelog = None;
            a.latest_changelog_version = None;
            a.channel_type = inst.default_addon_channel_type;
        }
        Ok(addon_list)
    }

    /// Rescan and reconcile with stored records, keeping user settings (`rescanInstallation`).
    pub async fn rescan(&self, inst: &WowInstallation) -> Result<Vec<Addon>> {
        let old = self.storage.addons(&inst.id)?;
        let mut new = self.scan_addons(inst, Some(&old)).await?;
        for n in new.iter_mut() {
            // Unknown addons are keyed by folder rather than by the always-empty external id.
            let existing = if n.is_unknown() {
                old.iter().find(|e| e.is_unknown() && e.installed_folder_list == n.installed_folder_list)
            } else {
                old.iter().find(|e| e.external_id == n.external_id && e.provider_name == n.provider_name)
            };
            if let Some(e) = existing {
                n.auto_update_enabled = e.auto_update_enabled;
                n.auto_update_notifications_enabled = e.auto_update_notifications_enabled;
                n.is_ignored = e.is_ignored || self.is_force_ignore(n);
                n.installed_at = e.installed_at.or(n.installed_at);
                n.channel_type = std::cmp::max_by_key(e.channel_type, n.channel_type, |c| *c as u8);
                n.external_latest_release_id = n.external_latest_release_id.clone().or(e.external_latest_release_id.clone());
                if n.id != e.id && !n.is_unknown() {
                    n.id = e.id.clone();
                }
            }
        }
        self.storage.save_addons(&inst.id, &new)?;
        Ok(new)
    }

    // ---- sync ----------------------------------------------------------------------------------

    /// Refresh latest-version data for every installation (`syncAllClients`).
    pub async fn sync_all(&self) -> Vec<SyncIssue> {
        let mut issues = Vec::new();
        for inst in self.installations().all().unwrap_or_default() {
            issues.extend(self.sync_installation(&inst).await);
        }
        issues
    }

    pub async fn sync_installation(&self, inst: &WowInstallation) -> Vec<SyncIssue> {
        let mut issues = Vec::new();
        for p in self.enabled_providers() {
            let addons: Vec<Addon> = self
                .storage
                .addons(&inst.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|a| !a.is_ignored && a.provider() == p.name())
                .collect();
            let ids: Vec<String> =
                addons.iter().filter_map(|a| a.external_id.clone()).filter(|i| !i.is_empty()).collect();
            if ids.is_empty() {
                continue;
            }
            match p.get_all(inst, &ids).await {
                Ok(r) => {
                    issues.extend(self.handle_sync_errors(inst, p.name(), &r.errors));
                    issues.extend(self.handle_sync_results(inst, r.search_results));
                }
                Err(e) => issues.push(SyncIssue {
                    provider: p.name().into(),
                    installation: inst.display_name.clone(),
                    addon: None,
                    message: format!("{e:#}"),
                }),
            }
        }
        self.update_companion(inst);
        issues
    }

    fn handle_sync_errors(&self, inst: &WowInstallation, provider: &str, errors: &[ProviderError]) -> Vec<SyncIssue> {
        let mut out = Vec::new();
        for err in errors {
            let mut name = None;
            if let (Some(id), Some(w)) = (&err.addon_id, err.warning) {
                if let Ok(Some(mut a)) = self.find_by_external(&inst.id, provider, id) {
                    a.warning_type = Some(w);
                    name = Some(a.name.clone());
                    let _ = self.save(&a);
                }
            }
            out.push(SyncIssue {
                provider: provider.into(),
                installation: inst.display_name.clone(),
                addon: name.or(err.addon_id.clone()),
                message: err.message.clone(),
            });
        }
        out
    }

    fn handle_sync_results(&self, inst: &WowInstallation, results: Vec<AddonSearchResult>) -> Vec<SyncIssue> {
        let mut out = Vec::new();
        let mut addons = self.storage.addons(&inst.id).unwrap_or_default();
        for r in results {
            let Some(a) = addons
                .iter_mut()
                .find(|a| a.external_id.as_deref() == Some(&r.external_id) && a.provider() == r.provider_name)
            else {
                continue;
            };
            let Some(lf) = r.latest_file(a.channel_type).cloned() else {
                a.warning_type = Some(AddonWarningType::NoProviderFiles);
                out.push(SyncIssue {
                    provider: r.provider_name.clone(),
                    installation: inst.display_name.clone(),
                    addon: Some(a.name.clone()),
                    message: "no files available on provider".into(),
                });
                continue;
            };
            a.summary = r.summary.clone().or(a.summary.take());
            a.thumbnail_url = Some(r.thumbnail_url.clone());
            a.latest_changelog = lf.changelog.clone().filter(|c| !c.is_empty()).or(a.latest_changelog.take());
            if matches!(a.warning_type, Some(AddonWarningType::MissingOnProvider | AddonWarningType::NoProviderFiles)) {
                a.warning_type = None;
            }
            a.screenshot_urls = r.screenshot_urls.clone();
            if !lf.download_url.is_empty() {
                a.download_url = Some(lf.download_url.clone());
            }
            if !r.funding_links.is_empty() {
                a.funding_links = r.funding_links.clone();
            }
            let same_release = lf.external_id.is_some() && lf.external_id == a.external_latest_release_id;
            let same_version = a.latest_version.as_deref() == Some(lf.version.as_str()) && a.released_at == lf.release_date;
            if same_release || same_version {
                continue;
            }
            a.latest_version = Some(lf.version.clone());
            a.released_at = lf.release_date;
            a.external_latest_release_id = lf.external_id.clone();
            a.name = r.name.clone();
            a.author = Some(r.author.clone());
            a.external_channel = Some(format!("{:?}", lf.channel_type));
            if !lf.game_version.is_empty() {
                a.game_version = vec![game_version(&lf.game_version)];
            }
            a.external_url = Some(r.external_url.clone());
        }
        let _ = self.storage.save_addons(&inst.id, &addons);
        out
    }

    // ---- install pipeline ----------------------------------------------------------------------

    /// New addon record for a search result file (`createAddon`).
    pub fn create_addon(sr: &AddonSearchResult, file: &AddonSearchResultFile, inst: &WowInstallation) -> Addon {
        Addon {
            id: uuid::Uuid::new_v4().to_string(),
            name: sr.name.clone(),
            thumbnail_url: Some(sr.thumbnail_url.clone()),
            latest_version: Some(file.version.clone()),
            client_type: Some(inst.client_type),
            external_id: Some(sr.external_id.clone()),
            game_version: if file.game_version.is_empty() { vec![] } else { vec![game_version(&file.game_version)] },
            author: Some(sr.author.clone()),
            download_url: Some(file.download_url.clone()),
            external_url: Some(sr.external_url.clone()),
            provider_name: Some(sr.provider_name.clone()),
            channel_type: inst.default_addon_channel_type,
            auto_update_enabled: inst.default_auto_update,
            auto_update_notifications_enabled: inst.default_auto_update,
            released_at: file.release_date,
            summary: sr.summary.clone(),
            screenshot_urls: sr.screenshot_urls.clone(),
            dependencies: file.dependencies.clone(),
            external_channel: Some(format!("{:?}", file.channel_type)),
            external_latest_release_id: file.external_id.clone(),
            funding_links: sr.funding_links.clone(),
            latest_changelog: file.changelog.clone(),
            latest_changelog_version: Some(file.version.clone()),
            installation_id: Some(inst.id.clone()),
            ..Default::default()
        }
    }

    /// Install a search result into an installation (`installPotentialAddon`).
    pub async fn install_search_result(
        self: &Arc<Self>,
        sr: &AddonSearchResult,
        inst: &WowInstallation,
        file: Option<&AddonSearchResultFile>,
        progress: ProgressFn,
    ) -> Result<Addon> {
        if self.is_installed(&inst.id, &sr.provider_name, &sr.external_id) {
            bail!("Addon already installed");
        }
        let file = match file {
            Some(f) => f.clone(),
            None => sr.latest_file(inst.default_addon_channel_type).cloned().ok_or_else(|| anyhow!("no files available"))?,
        };
        let addon = Self::create_addon(sr, &file, inst);
        self.install_addon(addon, progress, None).await
    }

    /// Install by provider id (`installBaseAddon`, used by import).
    pub async fn install_by_id(
        self: &Arc<Self>,
        provider: &str,
        external_id: &str,
        inst: &WowInstallation,
        progress: ProgressFn,
    ) -> Result<Addon> {
        if self.is_installed(&inst.id, provider, external_id) {
            bail!("Addon already installed");
        }
        let p = self.provider(provider).ok_or_else(|| anyhow!("unknown provider {provider}"))?;
        let sr = p.get_by_id(external_id, inst).await?.ok_or_else(|| anyhow!("addon {external_id} not found"))?;
        self.install_search_result(&sr, inst, None, progress).await
    }

    /// Install and then pull in required dependencies (`installAddon`).
    pub async fn install_addon(self: &Arc<Self>, addon: Addon, progress: ProgressFn, original: Option<Addon>) -> Result<Addon> {
        let installed = self.install_or_update(addon, progress.clone(), original).await?;
        self.save(&installed)?;
        let me = self.clone();
        let deps_for = installed.clone();
        me.install_dependencies(&deps_for, progress).await;
        Ok(installed)
    }

    /// Update in place, no dependency step (`updateAddon`).
    pub async fn update_addon(self: &Arc<Self>, addon: Addon, progress: ProgressFn) -> Result<Addon> {
        self.install_or_update(addon, progress, None).await
    }

    fn report(&self, progress: &ProgressFn, a: &Addon, s: InstallState, pct: u8) {
        {
            let mut active = self.active.lock().unwrap();
            if matches!(s, InstallState::Complete | InstallState::Error) {
                active.remove(&a.id);
            } else {
                active.insert(a.id.clone(), (s, pct));
            }
        }
        progress(a, s, pct);
    }

    async fn install_or_update(self: &Arc<Self>, mut addon: Addon, progress: ProgressFn, original: Option<Addon>) -> Result<Addon> {
        if addon.download_url.as_deref().unwrap_or("").is_empty() {
            bail!("Invalid addon download url for {}", addon.name);
        }
        self.report(&progress, &addon, InstallState::Pending, 0);
        let _slot = self.install_slots.acquire().await?;
        match self.process_install(&mut addon, &progress, original).await {
            Ok(()) => {
                if let Ok(inst) = self.installation(&addon) {
                    self.update_companion(&inst);
                }
                self.report(&progress, &addon, InstallState::Complete, 100);
                Ok(addon)
            }
            Err(e) => {
                self.report(&progress, &addon, InstallState::Error, 100);
                Err(e)
            }
        }
    }

    async fn download(&self, addon: &Addon, provider: &ProviderRef, dest: &Path) -> Result<()> {
        let auth = provider.download_auth().await.unwrap_or_default();
        let mut url = url::Url::parse(addon.download_url.as_deref().unwrap_or(""))?;
        for (k, v) in &auth.query_params {
            let pairs: Vec<(String, String)> = url.query_pairs().filter(|(qk, _)| qk != k).map(|(a, b)| (a.into(), b.into())).collect();
            url.query_pairs_mut().clear().extend_pairs(pairs).append_pair(k, v);
        }
        let mut req = self.net.client().get(url.as_str()).timeout(Duration::from_secs(300));
        for (k, v) in &auth.headers {
            req = req.header(k, v);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            bail!("Invalid response ({}): {}", status.as_u16(), addon.download_url.as_deref().unwrap_or(""));
        }
        let bytes = resp.bytes().await?;
        tokio::fs::write(dest, &bytes).await?;
        Ok(())
    }

    async fn process_install(self: &Arc<Self>, addon: &mut Addon, progress: &ProgressFn, original: Option<Addon>) -> Result<()> {
        tracing::info!(
            "[install] {} {} -> {}",
            addon.name,
            addon.installed_version.clone().unwrap_or_default(),
            addon.latest_version.clone().unwrap_or_default()
        );
        let inst = self.installation(addon)?;
        let provider = self.provider(addon.provider()).ok_or_else(|| anyhow!("provider not found: {}", addon.provider()))?;
        let downloads = self.downloads_dir();
        std::fs::create_dir_all(&downloads)?;
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let zip_path = downloads.join(format!("{nonce}-{}.zip", fsutil::slug(&addon.name)));
        let unzip_path = downloads.join(&nonce);

        let result: Result<()> = async {
            self.report(progress, addon, InstallState::Downloading, 25);
            let mut attempt = 0;
            loop {
                match self.download(addon, &provider, &zip_path).await {
                    Ok(()) => break,
                    Err(e) if attempt < DOWNLOAD_RETRIES => {
                        attempt += 1;
                        tracing::warn!("[install] download failed ({e:#}), retry {attempt}");
                        self.report(progress, addon, InstallState::Retry, 0);
                        // Refresh the download URL from the provider before retrying.
                        if let Ok(r) = provider.get_all(&inst, &[addon.external_id.clone().unwrap_or_default()]).await {
                            if let Some(lf) = r.search_results.first().and_then(|s| s.latest_file(addon.channel_type)) {
                                if !lf.download_url.is_empty() {
                                    addon.download_url = Some(lf.download_url.clone());
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(attempt - 1))).await;
                    }
                    Err(e) => return Err(e.context("download failed")),
                }
            }

            self.report(progress, addon, InstallState::BackingUp, 50);
            let addons_dir = inst.addons_path();
            std::fs::create_dir_all(&addons_dir)?;
            let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
            for f in &addon.installed_folder_list {
                let cur = addons_dir.join(f);
                let bak = addons_dir.join(format!("{f}-bak"));
                fsutil::remove_path(&bak)?;
                if cur.exists() {
                    std::fs::rename(&cur, &bak).or_else(|_| fsutil::copy_dir(&cur, &bak).and_then(|_| fsutil::remove_path(&cur)))?;
                    backups.push((cur, bak));
                }
            }

            self.report(progress, addon, InstallState::Installing, 75);
            let install: Result<Vec<String>> = (|| {
                fsutil::unzip(&zip_path, &unzip_path)?;
                let dirs = fsutil::list_dirs(&unzip_path)?;
                if dirs.is_empty() {
                    bail!("archive contains no addon folders");
                }
                for d in &dirs {
                    fsutil::copy_dir(&unzip_path.join(d), &addons_dir.join(d))?;
                }
                Ok(dirs)
            })();
            let dirs = match install {
                Ok(d) => {
                    for (_, bak) in &backups {
                        let _ = fsutil::remove_path(bak);
                    }
                    d
                }
                Err(e) => {
                    for (orig, bak) in &backups {
                        let _ = fsutil::remove_path(orig);
                        let _ = std::fs::rename(bak, orig);
                    }
                    return Err(e);
                }
            };

            addon.installed_external_release_id = addon.external_latest_release_id.clone();
            addon.installed_version = addon.latest_version.clone();
            addon.installed_at = Some(Utc::now());
            addon.set_folders_list(dirs.clone());
            addon.is_ignored = provider.capabilities().force_ignore;

            let tocs: Vec<Toc> = dirs
                .iter()
                .filter_map(|d| {
                    let folder = AddonFolder {
                        name: d.clone(),
                        path: addons_dir.join(d).to_string_lossy().into_owned(),
                        ignore_reason: None,
                        tocs: read_folder_tocs(&addons_dir.join(d)),
                        matching_addon: None,
                        modified_at: None,
                        cf_scan_results: None,
                        wowup_scan_results: None,
                    };
                    toc_for_folder(&folder, inst.client_type)
                })
                .collect();
            let gv: Vec<String> = tocs.iter().flat_map(|t| t.interface.iter().map(|i| game_version(i))).collect();
            if !gv.is_empty() {
                let mut seen = Vec::new();
                for g in gv {
                    if !seen.contains(&g) {
                        seen.push(g);
                    }
                }
                addon.game_version = seen;
            }
            if addon.author.as_deref().unwrap_or("").is_empty() {
                addon.author = tocs.iter().filter_map(|t| t.author.clone()).max_by_key(|a| a.len());
            }
            if addon.provider() == PROVIDER_ZIP {
                if let Some(t) = tocs.iter().filter_map(|t| t.title.clone()).max_by_key(|t| t.len()) {
                    addon.name = t;
                }
            }
            if let Some(main) = tocs.first() {
                self.set_external_ids(addon, main);
            }
            if let Some(orig) = &original {
                for id in &orig.external_ids {
                    if !addon.external_ids.contains(id) {
                        addon.external_ids.push(id.clone());
                    }
                }
                let ids = std::mem::take(&mut addon.external_ids);
                addon.external_ids = ids
                    .into_iter()
                    .filter(|e| self.provider(&e.provider_name).is_some_and(|p| p.is_valid_addon_id(&e.id)))
                    .collect();
            }
            self.save(addon)?;
            self.add_hub_external_id(addon, &inst).await;
            self.reconcile_addon_folders(addon)?;
            Ok(())
        }
        .await;

        let _ = fsutil::remove_path(&unzip_path);
        let _ = fsutil::remove_path(&zip_path);
        result
    }

    /// Learn the hub id of an addon installed elsewhere, by fingerprint (`addHubExternalId`).
    async fn add_hub_external_id(&self, addon: &mut Addon, inst: &WowInstallation) {
        if addon.provider() == PROVIDER_HUB
            || addon.installed_folder_list.is_empty()
            || addon.external_ids.iter().any(|e| e.provider_name == PROVIDER_HUB)
            || !self.prefs().is_provider_enabled(PROVIDER_HUB)
        {
            return;
        }
        let paths: Vec<PathBuf> = addon.installed_folder_list.iter().map(|f| inst.addons_path().join(f)).collect();
        let fps: Vec<String> = scan_folders(paths, HashesNeeded { wowup: true, curseforge: false })
            .await
            .into_iter()
            .filter_map(|(_, r)| r.wowup.map(|w| w.fingerprint))
            .collect();
        let hub = self.hub.read().unwrap().clone();
        if let Ok(map) = hub.addon_ids_by_fingerprints(&fps).await {
            if let Some(id) = map.values().next() {
                let mut ids = addon.external_ids.clone();
                self.insert_external_id(&mut ids, PROVIDER_HUB, Some(id));
                addon.external_ids = ids;
                let _ = self.save(addon);
            }
        }
    }

    /// Drop Unknown records that now share folders with the installed addon (`reconcileAddonFolders`).
    fn reconcile_addon_folders(&self, addon: &Addon) -> Result<()> {
        let inst_id = addon.installation_id.clone().unwrap_or_default();
        let mut all = self.storage.addons(&inst_id)?;
        all.retain(|o| {
            !(o.id != addon.id && o.is_unknown() && o.installed_folder_list.iter().any(|f| addon.installed_folder_list.contains(f)))
        });
        self.storage.save_addons(&inst_id, &all)
    }

    /// Install required dependencies that are not already present (`installDependencies`).
    async fn install_dependencies(self: &Arc<Self>, addon: &Addon, progress: ProgressFn) {
        let required: Vec<&AddonDependency> =
            addon.dependencies.iter().filter(|d| d.dep_type == AddonDependencyType::Required).collect();
        if required.is_empty() {
            return;
        }
        let Ok(inst) = self.installation(addon) else { return };
        let Some(provider) = self.provider(addon.provider()) else { return };
        let mut claimed: Vec<String> = Vec::new();
        let mut to_install: Vec<(String, Addon)> = Vec::new();
        for dep in required {
            let key = format!("{}|{}|{}", inst.id, provider.name(), dep.external_addon_id);
            if !self.pending_deps.lock().unwrap().insert(key.clone()) {
                continue;
            }
            let installed = self.storage.addons(&inst.id).unwrap_or_default();
            let present = installed.iter().any(|a| {
                (a.provider() == provider.name() && a.external_id.as_deref() == Some(&dep.external_addon_id))
                    || a.external_ids.iter().any(|e| e.provider_name == provider.name() && e.id == dep.external_addon_id)
            });
            let resolved = if present {
                None
            } else {
                match provider.get_by_id(&dep.external_addon_id, &inst).await {
                    Ok(Some(sr)) => sr.latest_file(inst.default_addon_channel_type).cloned().and_then(|lf| {
                        let installed_folders: Vec<&String> = installed.iter().flat_map(|a| a.installed_folder_list.iter()).collect();
                        if lf.folders.iter().any(|f| installed_folders.contains(&f) || claimed.contains(f)) {
                            None
                        } else {
                            claimed.extend(lf.folders.iter().cloned());
                            Some(Self::create_addon(&sr, &lf, &inst))
                        }
                    }),
                    _ => None,
                }
            };
            match resolved {
                Some(a) => to_install.push((key, a)),
                None => {
                    self.pending_deps.lock().unwrap().remove(&key);
                }
            }
        }
        for (key, dep) in to_install {
            let name = dep.name.clone();
            if let Err(e) = Box::pin(self.install_addon(dep, progress.clone(), None)).await {
                tracing::warn!("[deps] failed to install {name}: {e:#}");
            }
            self.pending_deps.lock().unwrap().remove(&key);
        }
    }

    pub async fn dependency_details(&self, addon: &Addon) -> Vec<DependencyInfo> {
        let mut out = Vec::new();
        let Ok(inst) = self.installation(addon) else { return out };
        let installed = self.storage.addons(&inst.id).unwrap_or_default();
        let provider = self.provider(addon.provider());
        for d in addon.dependencies.iter().filter(|d| d.dep_type == AddonDependencyType::Required) {
            if let Some(a) = installed.iter().find(|a| {
                a.provider() == addon.provider() && a.external_id.as_deref() == Some(&d.external_addon_id)
            }) {
                out.push(DependencyInfo { external_id: d.external_addon_id.clone(), name: a.name.clone(), installed: true });
                continue;
            }
            let name = match &provider {
                Some(p) => p.get_by_id(&d.external_addon_id, &inst).await.ok().flatten().map(|s| s.name),
                None => None,
            };
            out.push(DependencyInfo {
                external_id: d.external_addon_id.clone(),
                name: name.unwrap_or_else(|| d.external_addon_id.clone()),
                installed: false,
            });
        }
        out
    }

    // ---- remove --------------------------------------------------------------------------------

    /// Delete an addon's folders and record (`removeAddon`).
    pub fn remove_addon(&self, addon: &Addon, remove_dependencies: bool, remove_directories: bool) -> Result<()> {
        let inst = self.installation(addon)?;
        if remove_directories && !addon.installed_folder_list.is_empty() {
            let mut failed = 0;
            for f in &addon.installed_folder_list {
                if f.is_empty() || f.contains(['/', '\\']) || f == ".." {
                    failed += 1;
                    continue;
                }
                if let Err(e) = fsutil::remove_path(&inst.addons_path().join(f)) {
                    tracing::error!("[remove] {f}: {e}");
                    failed += 1;
                }
            }
            if failed == addon.installed_folder_list.len() {
                bail!("Failed to remove all directories");
            }
        }
        self.storage.remove_addon(&inst.id, &addon.id)?;
        if remove_dependencies {
            for d in &addon.dependencies {
                if let Ok(Some(dep)) = self.find_by_external(&inst.id, addon.provider(), &d.external_addon_id) {
                    let _ = self.remove_addon(&dep, false, true);
                }
            }
        }
        self.update_companion(&inst);
        Ok(())
    }

    /// Refresh the in-game update list in `wowup_data_addon` (`WowUpAddonService.updateForInstallation`).
    pub fn update_companion(&self, inst: &WowInstallation) {
        let addons = self.storage.addons(&inst.id).unwrap_or_default();
        if let Err(e) = crate::companion::write_data_addon(&inst.addons_path(), &addons) {
            tracing::warn!("[companion] {e:#}");
        }
    }

    // ---- per-addon settings --------------------------------------------------------------------

    /// Toggle ignore; ignoring also turns auto update off. Force-ignored providers cannot be unignored.
    pub fn set_ignored(&self, addon: &mut Addon, ignored: bool) -> Result<()> {
        if !ignored && self.is_force_ignore(addon) {
            bail!("{} addons cannot be unignored", addon.provider());
        }
        addon.is_ignored = ignored;
        if ignored {
            addon.auto_update_enabled = false;
        }
        self.save(addon)
    }

    pub fn set_auto_update(&self, addon: &mut Addon, enabled: bool) -> Result<()> {
        if addon.warning_type.is_some() {
            bail!("addon has a warning; auto update unavailable");
        }
        if enabled && self.is_force_ignore(addon) {
            bail!("{} addons cannot auto update", addon.provider());
        }
        addon.auto_update_enabled = enabled;
        addon.auto_update_notifications_enabled = enabled;
        if enabled {
            addon.is_ignored = false;
        }
        self.save(addon)
    }

    pub fn set_auto_update_notifications(&self, addon: &mut Addon, enabled: bool) -> Result<()> {
        addon.auto_update_notifications_enabled = enabled;
        self.save(addon)
    }

    pub fn set_channel(&self, addon: &mut Addon, channel: AddonChannelType) -> Result<()> {
        if !self.can_change_channel(addon) {
            bail!("{} does not support release channels", addon.provider());
        }
        addon.channel_type = channel;
        self.save(addon)
    }

    /// Apply an installation's default auto-update setting to all its addons.
    pub fn apply_installation_auto_update(&self, inst: &WowInstallation) -> Result<()> {
        let mut all = self.storage.addons(&inst.id)?;
        for a in all.iter_mut().filter(|a| !self.is_force_ignore(a)) {
            a.auto_update_enabled = inst.default_auto_update;
        }
        self.storage.save_addons(&inst.id, &all)
    }

    /// Switch an addon to another provider it is also published on (`setProvider`).
    pub async fn change_provider(self: &Arc<Self>, addon: &Addon, provider: &str, external_id: &str, progress: ProgressFn) -> Result<Addon> {
        let inst = self.installation(addon)?;
        let p = self.provider(provider).ok_or_else(|| anyhow!("provider not found"))?;
        if self.is_installed(&inst.id, provider, external_id) {
            bail!("error-addon-already-installed");
        }
        let sr = p.get_by_id(external_id, &inst).await?.ok_or_else(|| anyhow!("addon not found on {provider}"))?;
        let lf = sr.latest_file(inst.default_addon_channel_type).cloned().ok_or_else(|| anyhow!("no files on {provider}"))?;
        let new = Self::create_addon(&sr, &lf, &inst);
        let installed = self.install_addon(new, progress, Some(addon.clone())).await?;
        // The old record goes; its folders were replaced by the new install.
        self.storage.remove_addon(&inst.id, &addon.id)?;
        Ok(installed)
    }

    /// Auto-update pass: sync, then update every auto-update addon that needs it (`processAutoUpdates`).
    pub async fn process_auto_updates(self: &Arc<Self>) -> Result<Vec<Addon>> {
        self.sync_all().await;
        let mut updated = Vec::new();
        for inst in self.installations().all()? {
            for a in self.storage.addons(&inst.id)? {
                if !a.is_ignored && a.auto_update_enabled && a.needs_update() {
                    match self.update_addon(a, no_progress()).await {
                        Ok(a) => updated.push(a),
                        Err(e) => tracing::warn!("[auto-update] {e:#}"),
                    }
                }
            }
        }
        Ok(updated)
    }

    // ---- lookups for Get Addons / URLs / protocols ---------------------------------------------

    pub fn provider_for_url(&self, url: &url::Url) -> Option<ProviderRef> {
        self.all_providers().into_iter().find(|p| p.is_valid_addon_uri(url))
    }

    pub async fn search_by_url(&self, url_str: &str, inst: &WowInstallation) -> Result<AddonSearchResult> {
        let url = url::Url::parse(url_str.trim()).context("invalid URL")?;
        let p = self.provider_for_url(&url).ok_or_else(|| anyhow!("no provider handles {url}"))?;
        if !self.prefs().is_provider_enabled(p.name()) || !p.is_configured() {
            bail!("{} provider is disabled or not configured", p.name());
        }
        p.search_by_url(&url, inst).await?.ok_or_else(|| anyhow!("no addon found at {url}"))
    }

    pub async fn search_protocol(&self, protocol: &str) -> Result<ProtocolSearchResult> {
        let p = self
            .enabled_providers()
            .into_iter()
            .find(|p| p.is_valid_protocol(protocol))
            .ok_or_else(|| anyhow!("no enabled provider handles {protocol}"))?;
        p.search_protocol(protocol).await?.ok_or_else(|| anyhow!("nothing found for {protocol}"))
    }

    /// Search every enabled provider and merge results (Get Addons search box).
    pub async fn search(&self, query: &str, inst: &WowInstallation) -> (Vec<AddonSearchResult>, Vec<String>) {
        let futs = self.enabled_providers().into_iter().map(|p| {
            let q = query.to_string();
            let inst = inst.clone();
            async move { (p.name(), p.search(&q, &inst, inst.default_addon_channel_type).await) }
        });
        Self::merge(futures::future::join_all(futs).await)
    }

    pub async fn featured(&self, inst: &WowInstallation) -> (Vec<AddonSearchResult>, Vec<String>) {
        let futs = self.enabled_providers().into_iter().map(|p| {
            let inst = inst.clone();
            async move { (p.name(), p.get_featured(&inst, inst.default_addon_channel_type).await) }
        });
        Self::merge(futures::future::join_all(futs).await)
    }

    pub async fn category(&self, cat: AddonCategory, inst: &WowInstallation) -> (Vec<AddonSearchResult>, Vec<String>) {
        let futs = self.enabled_providers().into_iter().map(|p| {
            let inst = inst.clone();
            async move { (p.name(), p.get_category(cat, &inst).await) }
        });
        Self::merge(futures::future::join_all(futs).await)
    }

    fn merge(results: Vec<(&'static str, Result<Vec<AddonSearchResult>>)>) -> (Vec<AddonSearchResult>, Vec<String>) {
        let mut out = Vec::new();
        let mut errs = Vec::new();
        for (name, r) in results {
            match r {
                Ok(v) => out.extend(v),
                Err(e) => errs.push(format!("{name}: {e:#}")),
            }
        }
        out.sort_by(|a, b| b.download_count.unwrap_or(0).cmp(&a.download_count.unwrap_or(0)));
        (out, errs)
    }

    pub async fn description(&self, addon: &Addon) -> String {
        if addon.provider() == PROVIDER_ZIP || addon.is_unknown() {
            return self.describe_from_tocs(addon);
        }
        let (Ok(inst), Some(p)) = (self.installation(addon), self.provider(addon.provider())) else {
            return addon.summary.clone().unwrap_or_default();
        };
        let d = p.get_description(&inst, addon.external_id.as_deref().unwrap_or("")).await.unwrap_or_default();
        if d.is_empty() {
            addon.summary.clone().unwrap_or_default()
        } else {
            crate::providers::html_to_text(&d)
        }
    }

    pub async fn changelog(&self, addon: &Addon) -> String {
        if let Some(c) = addon.latest_changelog.as_ref().filter(|c| !c.is_empty()) {
            return crate::providers::html_to_text(c);
        }
        let (Ok(inst), Some(p)) = (self.installation(addon), self.provider(addon.provider())) else { return String::new() };
        if !p.capabilities().can_show_changelog {
            return String::new();
        }
        let r = p
            .get_changelog(
                &inst,
                addon.external_id.as_deref().unwrap_or(""),
                addon.external_latest_release_id.as_deref().unwrap_or(""),
            )
            .await
            .unwrap_or_default();
        crate::providers::html_to_text(&r)
    }

    /// Key/value dump of the main toc (Zip provider's `getDescription`).
    fn describe_from_tocs(&self, addon: &Addon) -> String {
        let Ok(inst) = self.installation(addon) else { return String::new() };
        let mut best: Option<Toc> = None;
        for f in &addon.installed_folder_list {
            if let Some(t) = read_folder_tocs(&inst.addons_path().join(f)).into_iter().find(|t| !t.interface.is_empty()) {
                let len = serde_json::to_string(&t).map(|s| s.len()).unwrap_or(0);
                if best.as_ref().is_none_or(|b| serde_json::to_string(b).map(|s| s.len()).unwrap_or(0) < len) {
                    best = Some(t);
                }
            }
        }
        let Some(t) = best else { return String::new() };
        let mut lines = Vec::new();
        let mut push = |k: &str, v: &Option<String>| {
            if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                lines.push(format!("{k}: {v}"));
            }
        };
        push("Title", &t.title);
        push("Author", &t.author);
        push("Version", &t.version);
        push("Notes", &t.notes);
        push("Website", &t.website);
        push("Dependencies", &t.dependencies);
        lines.push(format!("Interface: {}", t.interface.join(", ")));
        lines.join("\n")
    }
}

impl Addon {
    pub fn set_folders_list(&mut self, folders: Vec<String>) {
        self.installed_folders = Some(folders.join(","));
        self.installed_folder_list = folders;
    }
}
