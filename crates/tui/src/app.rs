// SPDX-License-Identifier: GPL-3.0-or-later
//! TUI state and actions. Long-running work runs on tokio tasks that report back through `Msg`.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use wowup_core::addons::{AddonService, InstallState, ProgressFn};
use wowup_core::installations::{resolve_user_path, Installations};
use wowup_core::models::*;
use wowup_core::wtf::{self, BackupInfo, WtfNode};
use wowup_core::{broker, fsutil};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    MyAddons,
    GetAddons,
    Options,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsTab {
    Clients,
    Providers,
    Application,
    WtfExplorer,
    Debug,
    About,
}

impl OptionsTab {
    pub const ALL: [OptionsTab; 6] = [
        OptionsTab::Clients,
        OptionsTab::Providers,
        OptionsTab::Application,
        OptionsTab::WtfExplorer,
        OptionsTab::Debug,
        OptionsTab::About,
    ];

    pub fn title(self) -> String {
        crate::i18n::t(match self {
            OptionsTab::Clients => "PAGES.OPTIONS.TABS.CLIENTS",
            OptionsTab::Providers => "PAGES.OPTIONS.TABS.ADDONS",
            OptionsTab::Application => "PAGES.OPTIONS.TABS.APPLICATION",
            OptionsTab::WtfExplorer => "PAGES.OPTIONS.TABS.WTF_EXPLORER",
            OptionsTab::Debug => "PAGES.OPTIONS.TABS.DEBUG",
            OptionsTab::About => "PAGES.OPTIONS.TABS.ABOUT",
        })
    }
}

pub enum Msg {
    News(Vec<wowup_core::news::NewsItem>),
    Addons(String, Vec<Addon>),
    Results(Vec<AddonSearchResult>, Vec<String>),
    Progress(String, String, InstallState, u8),
    Status(String),
    Error(String),
    Detail(String, String, Vec<String>),
    Backups(Vec<BackupInfo>),
    Wtf(Vec<WtfNode>),
    UrlResult(AddonSearchResult),
    Protocol(ProtocolSearchResult),
    Reload,
}

#[derive(Clone)]
pub enum Pending {
    RemoveAddons(Vec<String>),
    InstallUrl,
    InstallFoundUrl(Box<AddonSearchResult>),
    InstallProtocol(Box<ProtocolSearchResult>),
    AddClient,
    RemoveClient(String),
    SetLabel(String),
    SetKey(&'static str),
    SetInterval,
    SetBackupCount,
    Import,
    ApplyImport(String),
    RestoreBackup(String),
    DeleteBackup(String),
    Channel(Vec<String>),
    DefaultChannel(String),
    Provider(String),
    Category,
    OpenUrl(String),
    Menu,
    NewsOpen,
    Language,
    EnableSymlinks,
}

pub enum Modal {
    Confirm { text: String, action: Pending },
    Input { title: String, value: String, action: Pending, secret: bool },
    Pick { title: String, items: Vec<(String, String)>, idx: usize, action: Pending },
    Detail(Box<DetailView>),
    Text { title: String, body: String, scroll: u16 },
    Backups { idx: usize },
    Help,
}

pub struct DetailView {
    pub addon: Option<Addon>,
    pub result: Option<AddonSearchResult>,
    pub tab: usize,
    pub description: String,
    pub changelog: String,
    pub deps: Vec<String>,
    pub scroll: u16,
}

pub struct App {
    pub svc: Arc<AddonService>,
    pub tx: UnboundedSender<Msg>,
    pub tab: Tab,
    pub options_tab: OptionsTab,
    pub installations: Vec<WowInstallation>,
    pub inst_idx: usize,
    pub addons: Vec<Addon>,
    pub addon_cursor: usize,
    pub selected: BTreeSet<String>,
    pub filter: String,
    pub filtering: bool,
    pub results: Vec<AddonSearchResult>,
    pub result_cursor: usize,
    pub query: String,
    pub searching: bool,
    pub category: Option<AddonCategory>,
    pub option_cursor: usize,
    pub status: String,
    pub errors: Vec<String>,
    pub busy: usize,
    pub progress: std::collections::BTreeMap<String, (String, InstallState, u8)>,
    pub modal: Option<Modal>,
    pub backups: Vec<BackupInfo>,
    pub wtf_nodes: Vec<WtfNode>,
    pub sort: (usize, bool),
    pub quit: bool,
    pub last_title: String,
    pub window: Option<crate::window::TerminalWindow>,
    pub last_auto_update: std::time::Instant,
}

pub const MY_ADDON_COLUMNS: [&str; 7] = ["Addon", "Status", "Latest", "Released", "Game", "Provider", "Author"];

impl App {
    pub fn new(svc: Arc<AddonService>, tx: UnboundedSender<Msg>) -> Self {
        let mut app = Self {
            svc,
            tx,
            tab: Tab::MyAddons,
            options_tab: OptionsTab::Clients,
            installations: vec![],
            inst_idx: 0,
            addons: vec![],
            addon_cursor: 0,
            selected: BTreeSet::new(),
            filter: String::new(),
            filtering: false,
            results: vec![],
            result_cursor: 0,
            query: String::new(),
            searching: false,
            category: None,
            option_cursor: 0,
            status: "Welcome to WowUp.rs — press ? for help".into(),
            errors: vec![],
            busy: 0,
            progress: Default::default(),
            modal: None,
            backups: vec![],
            wtf_nodes: vec![],
            sort: (0, true),
            last_title: String::new(),
            window: None,
            quit: false,
            last_auto_update: std::time::Instant::now(),
        };
        if let Some((col, asc)) = app.svc.prefs().my_addons_sort {
            if let Some(i) = MY_ADDON_COLUMNS.iter().position(|c| *c == col) {
                app.sort = (i, asc);
            }
        }
        app.reload_installations();
        if app.installations.is_empty() {
            if let Ok(added) = Installations::new(&app.svc.storage).import_detected() {
                if !added.is_empty() {
                    app.status = format!("Detected {} WoW installation(s)", added.len());
                }
            }
            app.reload_installations();
        }
        if app.installations.is_empty() {
            app.tab = Tab::Options;
            app.status = "No WoW installation found — press `a` to add one".into();
        }
        app
    }

    pub fn reload_installations(&mut self) {
        self.installations = Installations::new(&self.svc.storage).all().unwrap_or_default();
        if let Some(i) = self.installations.iter().position(|i| i.selected) {
            self.inst_idx = i;
        }
        self.inst_idx = self.inst_idx.min(self.installations.len().saturating_sub(1));
    }

    pub fn inst(&self) -> Option<WowInstallation> {
        self.installations.get(self.inst_idx).cloned()
    }

    // ---- background helpers --------------------------------------------------------------------

    fn spawn<F, Fut>(&mut self, label: &str, f: F)
    where
        F: FnOnce(Arc<AddonService>, UnboundedSender<Msg>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.busy += 1;
        self.status = format!("{label}…");
        let svc = self.svc.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(e) = f(svc, tx.clone()).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
            let _ = tx.send(Msg::Status(String::new()));
        });
    }

    pub fn progress_fn(&self) -> ProgressFn {
        let tx = self.tx.clone();
        Arc::new(move |a: &Addon, s: InstallState, p: u8| {
            let _ = tx.send(Msg::Progress(a.id.clone(), a.name.clone(), s, p));
        })
    }

    pub fn load_addons(&mut self, rescan: bool) {
        let Some(inst) = self.inst() else { return };
        self.spawn(if rescan { "Scanning addon folders" } else { "Loading addons" }, move |svc, tx| async move {
            let addons = svc.get_addons(&inst, rescan).await?;
            tx.send(Msg::Addons(inst.id.clone(), addons)).ok();
            Ok(())
        });
    }

    pub fn check_updates(&mut self, all_clients: bool) {
        let insts: Vec<WowInstallation> = if all_clients { self.installations.clone() } else { self.inst().into_iter().collect() };
        let current = self.inst();
        self.spawn("Checking for updates", move |svc, tx| async move {
            for inst in &insts {
                for issue in svc.sync_installation(inst).await {
                    tx.send(Msg::Error(format!("[{}] {}: {}", issue.provider, issue.addon.unwrap_or_default(), issue.message))).ok();
                }
            }
            if let Some(inst) = current {
                tx.send(Msg::Addons(inst.id.clone(), svc.get_addons(&inst, false).await?)).ok();
            }
            Ok(())
        });
    }

    pub fn auto_update_tick(&mut self) {
        let interval = self.svc.prefs().auto_update_interval_minutes.max(5) * 60;
        if self.last_auto_update.elapsed().as_secs() < interval || self.installations.is_empty() {
            return;
        }
        self.last_auto_update = std::time::Instant::now();
        let notify = self.svc.prefs().enable_system_notifications;
        self.spawn("Auto-updating", move |svc, tx| async move {
            let updated = svc.process_auto_updates().await?;
            if !updated.is_empty() {
                let names: Vec<String> =
                    updated.iter().filter(|a| a.auto_update_notifications_enabled).map(|a| a.name.clone()).collect();
                if notify && !names.is_empty() {
                    let body = if names.join(", ").len() > 60 {
                        format!("{} addons were updated", names.len())
                    } else {
                        names.join(", ")
                    };
                    crate::notify("WowUp.rs auto update", &body);
                }
                tx.send(Msg::Status(format!("Auto-updated {} addon(s)", updated.len()))).ok();
                tx.send(Msg::Reload).ok();
            }
            Ok(())
        });
    }

    // ---- My Addons -----------------------------------------------------------------------------

    pub fn visible_addons(&self) -> Vec<&Addon> {
        let f = self.filter.to_lowercase();
        let mut v: Vec<&Addon> = self
            .addons
            .iter()
            .filter(|a| {
                f.is_empty()
                    || a.name.to_lowercase().contains(&f)
                    || a.author.as_deref().unwrap_or("").to_lowercase().contains(&f)
                    || a.provider().to_lowercase().contains(&f)
            })
            .collect();
        let (col, asc) = self.sort;
        v.sort_by(|a, b| {
            let o = match col {
                1 => status_rank(a).cmp(&status_rank(b)),
                2 => a.latest_version.cmp(&b.latest_version),
                3 => a.released_at.cmp(&b.released_at),
                4 => a.game_version.first().cmp(&b.game_version.first()),
                5 => a.provider().cmp(b.provider()),
                6 => a.author.cmp(&b.author),
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if asc {
                o
            } else {
                o.reverse()
            }
        });
        v
    }

    /// Selected rows, or the row under the cursor if nothing is selected.
    pub fn targets(&self) -> Vec<Addon> {
        let vis = self.visible_addons();
        if !self.selected.is_empty() {
            return vis.into_iter().filter(|a| self.selected.contains(&a.id)).cloned().collect();
        }
        vis.get(self.addon_cursor).map(|a| vec![(*a).clone()]).unwrap_or_default()
    }

    pub fn update_targets(&mut self, all: bool) {
        let targets: Vec<Addon> = if all {
            self.addons.iter().filter(|a| a.needs_update()).cloned().collect()
        } else {
            self.targets().into_iter().filter(|a| a.needs_update() || (self.svc.can_reinstall(a) && a.needs_install())).collect()
        };
        if targets.is_empty() {
            self.status = "Nothing to update".into();
            return;
        }
        self.run_installs(targets, false);
    }

    pub fn update_all_clients(&mut self) {
        let insts = self.installations.clone();
        let progress = self.progress_fn();
        self.spawn("Updating all clients", move |svc, tx| async move {
            for inst in insts {
                svc.sync_installation(&inst).await;
                for a in svc.get_addons(&inst, false).await? {
                    if a.needs_update() {
                        if let Err(e) = svc.update_addon(a.clone(), progress.clone()).await {
                            tx.send(Msg::Error(format!("{}: {e:#}", a.name))).ok();
                        }
                    }
                }
            }
            tx.send(Msg::Reload).ok();
            Ok(())
        });
    }

    pub fn run_installs(&mut self, targets: Vec<Addon>, reinstall: bool) {
        let progress = self.progress_fn();
        let n = targets.len();
        self.spawn(&format!("{} {n} addon(s)", if reinstall { "Reinstalling" } else { "Updating" }), move |svc, tx| async move {
            let mut handles = Vec::new();
            for a in targets {
                let svc = svc.clone();
                let progress = progress.clone();
                let tx = tx.clone();
                handles.push(tokio::spawn(async move {
                    let r = if reinstall { svc.install_addon(a.clone(), progress, None).await } else { svc.update_addon(a.clone(), progress).await };
                    if let Err(e) = r {
                        tx.send(Msg::Error(format!("{}: {e:#}", a.name))).ok();
                    }
                }));
            }
            for h in handles {
                let _ = h.await;
            }
            tx.send(Msg::Status(format!("Finished {n} addon(s)"))).ok();
            tx.send(Msg::Reload).ok();
            Ok(())
        });
    }

    pub fn toggle_ignore(&mut self) {
        let targets = self.targets();
        let new_val = targets.iter().all(|a| !a.is_ignored);
        for mut a in targets {
            if let Err(e) = self.svc.set_ignored(&mut a, new_val) {
                self.errors.push(format!("{}: {e:#}", a.name));
            }
        }
        self.load_addons(false);
    }

    pub fn toggle_auto_update(&mut self) {
        let targets = self.targets();
        let new_val = targets.iter().all(|a| !a.auto_update_enabled);
        for mut a in targets {
            if let Err(e) = self.svc.set_auto_update(&mut a, new_val) {
                self.errors.push(format!("{}: {e:#}", a.name));
            }
        }
        self.load_addons(false);
    }

    pub fn toggle_notifications(&mut self) {
        if !self.svc.prefs().enable_system_notifications {
            self.status = "System notifications are off (Options → Application)".into();
            return;
        }
        let targets: Vec<Addon> = self.targets().into_iter().filter(|a| a.auto_update_enabled && !a.is_ignored).collect();
        let new_val = targets.iter().all(|a| !a.auto_update_notifications_enabled);
        for mut a in targets {
            let _ = self.svc.set_auto_update_notifications(&mut a, new_val);
        }
        self.load_addons(false);
    }

    pub fn open_channel_picker(&mut self) {
        let targets: Vec<Addon> = self.targets().into_iter().filter(|a| self.svc.can_change_channel(a)).collect();
        if targets.is_empty() {
            self.status = "Release channels are not supported for this provider".into();
            return;
        }
        self.modal = Some(Modal::Pick {
            title: "Release channel".into(),
            items: crate::ui::channel_items(),
            idx: targets[0].channel_type as usize,
            action: Pending::Channel(targets.iter().map(|a| a.id.clone()).collect()),
        });
    }

    pub fn open_provider_picker(&mut self) {
        let Some(a) = self.targets().into_iter().next() else { return };
        let items: Vec<(String, String)> = a
            .external_ids
            .iter()
            .filter(|e| e.provider_name != a.provider())
            .filter(|e| self.svc.enabled_providers().iter().any(|p| p.name() == e.provider_name))
            .map(|e| (format!("{} ({})", e.provider_name, e.id), format!("{}|{}", e.provider_name, e.id)))
            .collect();
        if items.is_empty() {
            self.status = format!("{} is not known on any other enabled provider", a.name);
            return;
        }
        self.modal = Some(Modal::Pick { title: format!("Switch {} to…", a.name), items, idx: 0, action: Pending::Provider(a.id.clone()) });
    }

    pub fn ask_remove(&mut self) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let names: Vec<String> = targets.iter().map(|a| a.name.clone()).collect();
        let ids: Vec<String> = targets.iter().map(|a| a.id.clone()).collect();
        let has_deps = targets.iter().any(|a| a.dependencies.iter().any(|d| d.dep_type == AddonDependencyType::Required));
        self.modal = Some(Modal::Confirm {
            text: format!(
                "Remove {}?\nThis deletes its folders from AddOns.{}",
                names.join(", "),
                if has_deps { "\n\nPress `d` instead of `y` to also remove required dependencies." } else { "" }
            ),
            action: Pending::RemoveAddons(ids),
        });
    }

    pub fn reinstall(&mut self) {
        let targets: Vec<Addon> = self.targets().into_iter().filter(|a| self.svc.can_reinstall(a)).collect();
        if targets.is_empty() {
            self.status = "Selected addons cannot be reinstalled".into();
            return;
        }
        self.run_installs(targets, true);
    }

    pub fn open_addon_folder(&mut self) {
        let (Some(a), Some(inst)) = (self.targets().into_iter().next(), self.inst()) else { return };
        let p = a.installed_folder_list.first().map(|f| inst.addons_path().join(f)).unwrap_or_else(|| inst.addons_path());
        let _ = fsutil::open_in_file_manager(&p);
    }

    pub fn open_addon_web(&mut self) {
        if let Some(url) = self.targets().into_iter().next().and_then(|a| a.external_url).filter(|u| !u.is_empty()) {
            self.open_link(url);
        }
    }

    /// Open an external link, confirming first unless its domain is trusted (`LinkService`).
    pub fn open_link(&mut self, url: String) {
        if self.svc.prefs().is_trusted_url(&url) {
            let _ = fsutil::open_url(&url);
        } else {
            self.modal = Some(Modal::Confirm {
                text: format!("Open this link in your browser?\n\n{url}\n\n[y] open   [t] open and always trust this domain   [n] cancel"),
                action: Pending::OpenUrl(url),
            });
        }
    }

    /// Persist the My Addons sort (`my_addons_sort_order`).
    pub fn set_sort(&mut self, sort: (usize, bool)) {
        self.sort = sort;
        let mut p = self.svc.prefs();
        p.my_addons_sort = Some((MY_ADDON_COLUMNS[sort.0].to_string(), sort.1));
        let _ = self.svc.save_prefs(p);
    }

    fn detail_tab(&self) -> usize {
        let p = self.svc.prefs();
        if p.keep_detail_tab { p.last_detail_tab.min(3) } else { 0 }
    }

    pub fn open_detail_for_addon(&mut self) {
        let Some(a) = self.targets().into_iter().next() else { return };
        self.modal = Some(Modal::Detail(Box::new(DetailView {
            addon: Some(a.clone()),
            result: None,
            tab: self.detail_tab(),
            description: "Loading…".into(),
            changelog: String::new(),
            deps: vec![],
            scroll: 0,
        })));
        self.spawn("Loading details", move |svc, tx| async move {
            let d = svc.description(&a).await;
            let c = svc.changelog(&a).await;
            let deps = svc
                .dependency_details(&a)
                .await
                .into_iter()
                .map(|d| format!("{} — {}", d.name, if d.installed { "installed" } else { "missing" }))
                .chain(a.missing_dependencies.iter().map(|m| format!("{m} — missing (toc dependency)")))
                .collect();
            tx.send(Msg::Detail(d, c, deps)).ok();
            Ok(())
        });
    }

    pub fn export(&mut self) {
        let Some(inst) = self.inst() else { return };
        match broker::export(&inst, &self.addons) {
            Ok(s) => {
                let copied = crate::copy_to_clipboard(&s);
                self.modal = Some(Modal::Text {
                    title: format!("Export{}", if copied { " (copied to clipboard)" } else { "" }),
                    body: s,
                    scroll: 0,
                });
            }
            Err(e) => self.errors.push(format!("{e:#}")),
        }
    }

    pub fn open_backups(&mut self) {
        let Some(inst) = self.inst() else { return };
        self.backups = wtf::list_backups(self.svc.storage.root(), &inst);
        self.modal = Some(Modal::Backups { idx: 0 });
    }

    pub fn create_backup(&mut self) {
        let Some(inst) = self.inst() else { return };
        let keep = self.svc.prefs().wtf_backup_count;
        self.spawn("Backing up WTF", move |svc, tx| async move {
            let root = svc.storage.root().to_path_buf();
            let inst2 = inst.clone();
            let path = tokio::task::spawn_blocking(move || wtf::create_backup(&root, &inst2)).await??;
            if keep > 0 {
                wtf::prune_backups(svc.storage.root(), &inst, keep);
            }
            tx.send(Msg::Backups(wtf::list_backups(svc.storage.root(), &inst))).ok();
            tx.send(Msg::Status(format!("Created {}", path.file_name().unwrap_or_default().to_string_lossy()))).ok();
            Ok(())
        });
    }

    // ---- Get Addons ----------------------------------------------------------------------------

    pub fn load_results(&mut self) {
        let Some(inst) = self.inst() else { return };
        let q = self.query.clone();
        let cat = self.category;
        self.results.clear();
        self.result_cursor = 0;
        let label = if !q.is_empty() { format!("Searching for “{q}”") } else { "Loading featured addons".into() };
        self.spawn(&label, move |svc, tx| async move {
            let (r, e) = if !q.is_empty() {
                svc.search(&q, &inst).await
            } else if let Some(c) = cat {
                svc.category(c, &inst).await
            } else {
                svc.featured(&inst).await
            };
            tx.send(Msg::Results(r, e)).ok();
            Ok(())
        });
    }

    pub fn install_result(&mut self, sr: AddonSearchResult) {
        let Some(inst) = self.inst() else { return };
        if self.svc.is_installed(&inst.id, &sr.provider_name, &sr.external_id) {
            self.status = format!("{} is already installed", sr.name);
            return;
        }
        let progress = self.progress_fn();
        self.spawn(&format!("Installing {}", sr.name), move |svc, tx| async move {
            let a = svc.install_search_result(&sr, &inst, None, progress).await?;
            tx.send(Msg::Status(format!("Installed {}", a.name))).ok();
            tx.send(Msg::Reload).ok();
            Ok(())
        });
    }

    pub fn open_detail_for_result(&mut self) {
        let Some(sr) = self.results.get(self.result_cursor).cloned() else { return };
        let mut changelog = sr
            .latest_file(self.inst().map(|i| i.default_addon_channel_type).unwrap_or_default())
            .and_then(|f| f.changelog.clone())
            .unwrap_or_default();
        changelog = wowup_core::providers::html_to_text(&changelog);
        let desc = sr.summary.clone().unwrap_or_default();
        let fetch = (sr.external_id.clone(), sr.provider_name.clone());
        self.modal = Some(Modal::Detail(Box::new(DetailView {
            addon: None,
            result: Some(sr),
            tab: self.detail_tab(),
            description: desc,
            changelog,
            deps: vec![],
            scroll: 0,
        })));
        let Some(inst) = self.inst() else { return };
        self.spawn("Loading details", move |svc, tx| async move {
            if let Some(p) = svc.provider(&fetch.1) {
                let d = p.get_description(&inst, &fetch.0).await.unwrap_or_default();
                if !d.is_empty() {
                    tx.send(Msg::Detail(wowup_core::providers::html_to_text(&d), String::new(), vec![])).ok();
                }
            }
            Ok(())
        });
    }

    pub fn lookup_url(&mut self, url: String) {
        let Some(inst) = self.inst() else { return };
        self.spawn("Looking up URL", move |svc, tx| async move {
            let sr = svc.search_by_url(&url, &inst).await?;
            tx.send(Msg::UrlResult(sr)).ok();
            Ok(())
        });
    }

    pub fn lookup_protocol(&mut self, url: String) {
        self.spawn("Resolving link", move |svc, tx| async move {
            let p = svc.search_protocol(&url).await?;
            tx.send(Msg::Protocol(p)).ok();
            Ok(())
        });
    }

    // ---- Options -------------------------------------------------------------------------------

    pub fn detect_clients(&mut self) {
        match Installations::new(&self.svc.storage).import_detected() {
            Ok(a) => self.status = format!("Detected {} new installation(s)", a.len()),
            Err(e) => self.errors.push(format!("{e:#}")),
        }
        self.reload_installations();
    }

    pub fn load_wtf(&mut self) {
        let Some(inst) = self.inst() else { return };
        let folders: Vec<String> = self.addons.iter().flat_map(|a| a.installed_folder_list.clone()).collect();
        self.spawn("Reading WTF folder", move |_svc, tx| async move {
            let nodes = tokio::task::spawn_blocking(move || wtf::explore(&inst, &folders)).await?;
            tx.send(Msg::Wtf(nodes)).ok();
            Ok(())
        });
    }

    // ---- Debug (Options → Debug) ---------------------------------------------------------------

    /// Write a diagnostic snapshot to the logs folder (`Dump Debug Data`). Keys are redacted.
    pub fn dump_debug_data(&mut self) {
        let mut prefs = serde_json::to_value(self.svc.prefs()).unwrap_or_default();
        for k in ["curseforgeApiKey", "wagoApiKey", "githubToken"] {
            if prefs[k].is_string() {
                prefs[k] = "<redacted>".into();
            }
        }
        let installs: Vec<serde_json::Value> = self
            .installations
            .iter()
            .map(|i| {
                let addons = self.svc.storage.addons(&i.id).unwrap_or_default();
                let mut by_provider = std::collections::BTreeMap::<String, usize>::new();
                for a in &addons {
                    *by_provider.entry(a.provider().to_string()).or_default() += 1;
                }
                serde_json::json!({
                    "id": i.id, "clientType": format!("{:?}", i.client_type), "location": i.location,
                    "addonCount": addons.len(), "updatesAvailable": addons.iter().filter(|a| a.needs_update()).count(),
                    "byProvider": by_provider,
                    "warnings": addons.iter().filter(|a| a.warning_type.is_some()).map(|a| format!("{}: {:?}", a.name, a.warning_type)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let providers: Vec<serde_json::Value> = self
            .svc
            .all_providers()
            .iter()
            .map(|p| serde_json::json!({"name": p.name(), "enabled": self.svc.prefs().is_provider_enabled(p.name()), "configured": p.is_configured()}))
            .collect();
        let dump = serde_json::json!({
            "app": "WowUp.rs", "version": env!("CARGO_PKG_VERSION"), "parityCommit": crate::PARITY_COMMIT.trim(),
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "dataDir": self.svc.storage.root(),
            "installations": installs, "providers": providers, "preferences": prefs,
            "recentErrors": self.errors, "metrics": process_metrics(),
        });
        let dir = self.svc.storage.log_dir();
        let path = dir.join(format!("debug-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let body = serde_json::to_string_pretty(&dump).unwrap_or_default();
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &body)) {
            Ok(()) => {
                tracing::info!("debug data written to {}", path.display());
                self.modal = Some(Modal::Text { title: format!("Debug data → {}", path.display()), body, scroll: 0 });
            }
            Err(e) => self.errors.push(format!("{e}")),
        }
    }

    // ---- Zoom / log viewer ---------------------------------------------------------------------

    /// Zoom in (+1), out (-1) or reset (0) through upstream's zoom steps by scaling the terminal font.
    pub fn zoom(&mut self, dir: i32) {
        let Some(w) = &self.window else {
            self.status = "Zoom works in Terminal.app; in other terminals use their own font size keys".into();
            return;
        };
        let mut p = self.svc.prefs();
        let base = match p.base_font_size {
            Some(b) => b,
            None => {
                let b = w.font_size().unwrap_or(12.0) / p.zoom_factor;
                p.base_font_size = Some(b);
                b
            }
        };
        p.zoom_factor = if dir == 0 { 1.0 } else { crate::window::step_zoom(p.zoom_factor, dir) };
        w.set_font_size(base * p.zoom_factor);
        self.status = format!("Zoom {}%", (p.zoom_factor * 100.0).round());
        let _ = self.svc.save_prefs(p);
    }

    /// The tail of this app's log, the terminal counterpart of DevTools' console.
    pub fn view_log(&mut self) {
        let path = self.svc.storage.log_dir().join("wowup-tui.log");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| format!("{}: {e}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(400)..].join("\n");
        let scroll = tail.lines().count().saturating_sub(25) as u16;
        self.modal = Some(Modal::Text { title: format!("Log — {}", path.display()), body: tail, scroll });
    }

    /// Rebuild and reinstall from source in a new Terminal window (`wowup self-update`), the
    /// counterpart of upstream's app updater.
    pub fn self_update(&mut self) {
        let cli = std::env::current_exe()
            .and_then(|e| e.canonicalize())
            .map(|e| e.with_file_name("wowup"))
            .unwrap_or_else(|_| "wowup".into());
        let cmd = format!("'{}' self-update", cli.display());
        let ok = std::process::Command::new("osascript")
            .args(["-e", &format!("tell application \"Terminal\" to do script \"{}\"", cmd.replace('"', "\\\""))])
            .spawn()
            .is_ok();
        self.status = if ok { "Updating in a new Terminal window; restart WowUp.rs when it finishes".into() } else { format!("Run {cmd} to update") };
    }

    // ---- App menu ------------------------------------------------------------------------------

    /// Every action in one list, the terminal counterpart of the app menu and toolbar buttons.
    pub fn open_menu(&mut self) {
        let t = crate::i18n::t;
        let items: Vec<(String, String)> = [
            (t("PAGES.MY_ADDONS.CHECK_UPDATES_BUTTON"), "check"),
            (t("PAGES.MY_ADDONS.UPDATE_ALL_BUTTON"), "update_all"),
            (t("PAGES.MY_ADDONS.UPDATE_ALL_CONTEXT_MENU.UPDATE_ALL_CLIENTS_BUTTON"), "update_all_clients"),
            (t("PAGES.MY_ADDONS.RESCAN_FOLDERS_BUTTON"), "rescan"),
            (t("PAGES.GET_ADDONS.INSTALL_FROM_URL_BUTTON"), "install_url"),
            (t("PAGES.MY_ADDONS.IMPORT_EXPORT_ADDONS_BUTTON") + " (export)", "export"),
            (t("PAGES.MY_ADDONS.IMPORT_EXPORT_ADDONS_BUTTON") + " (import)", "import"),
            ("WTF Backup".to_string(), "backups"),
            (t("PAGES.GET_ADDONS.ADDON_CATEGORIES_BUTTON"), "categories"),
            (t("PAGES.OPTIONS.WOW.ADD_CLIENT_BUTTON"), "add_client"),
            ("Re-Scan WoW clients".to_string(), "detect"),
            (t("PAGES.OPTIONS.DEBUG.LOG_FILES_BUTTON"), "logs"),
            (t("PAGES.OPTIONS.DEBUG.CONFIG_FILES_BUTTON"), "config"),
            (t("PAGES.OPTIONS.DEBUG.DEBUG_DATA_BUTTON"), "dump"),
            ("View log".to_string(), "log"),
            ("Zoom in (+)".to_string(), "zoom_in"),
            ("Zoom out (-)".to_string(), "zoom_out"),
            ("Reset zoom (=)".to_string(), "zoom_reset"),
            (crate::i18n::t("PAGES.HOME.NEWS_TAB_TITLE"), "news"),
            ("Update WowUp.rs (rebuild from source)".to_string(), "self_update"),
            ("Errors".to_string(), "errors"),
            ("Help".to_string(), "help"),
            (t("PAGES.HOME.GUIDE_TAB_TITLE"), "guide"),
            ("Quit".to_string(), "quit"),
        ]
        .into_iter()
        .map(|(l, c)| (l, c.to_string()))
        .collect();
        self.modal = Some(Modal::Pick { title: "Menu".into(), items, idx: 0, action: Pending::Menu });
    }

    fn run_menu(&mut self, code: &str) {
        match code {
            "check" => self.check_updates(false),
            "update_all" => self.update_targets(true),
            "update_all_clients" => self.update_all_clients(),
            "rescan" => self.load_addons(true),
            "install_url" => {
                self.modal = Some(Modal::Input {
                    title: "Install from URL (WowInterface, GitHub, CurseForge, Wago, .zip) or wowup:// link".into(),
                    value: String::new(),
                    action: Pending::InstallUrl,
                    secret: false,
                })
            }
            "export" => self.export(),
            "import" => {
                self.modal = Some(Modal::Input { title: "Paste a WowUp import string".into(), value: String::new(), action: Pending::Import, secret: false })
            }
            "backups" => self.open_backups(),
            "categories" => {
                self.tab = Tab::GetAddons;
                self.modal = Some(Modal::Pick {
                    title: crate::i18n::t("PAGES.GET_ADDONS.ADDON_CATEGORIES_MENU_TITLE"),
                    items: AddonCategory_ALL.iter().enumerate().map(|(i, c)| (crate::ui::category_text(*c), i.to_string())).collect(),
                    idx: 0,
                    action: Pending::Category,
                });
            }
            "add_client" => {
                self.modal = Some(Modal::Input {
                    title: "Path to a WoW client folder, game .app/.exe, or the World of Warcraft folder".into(),
                    value: String::new(),
                    action: Pending::AddClient,
                    secret: false,
                })
            }
            "detect" => self.detect_clients(),
            "logs" => {
                let _ = fsutil::open_in_file_manager(&self.svc.storage.log_dir());
            }
            "config" => {
                let _ = fsutil::open_in_file_manager(self.svc.storage.root());
            }
            "dump" => self.dump_debug_data(),
            "news" => self.spawn("Loading news", |svc, tx| async move {
                tx.send(Msg::News(wowup_core::news::load(&svc.net).await)).ok();
                Ok(())
            }),
            "self_update" => self.self_update(),
            "log" => self.view_log(),
            "zoom_in" => self.zoom(1),
            "zoom_out" => self.zoom(-1),
            "zoom_reset" => self.zoom(0),
            "errors" => {
                self.modal = Some(Modal::Text { title: "Errors".into(), body: self.errors.join("\n"), scroll: 0 });
                self.errors.clear();
            }
            "help" => self.modal = Some(Modal::Help),
            "guide" => self.open_link("https://wowup.io/guide".into()),
            "quit" => self.quit = true,
            _ => {}
        }
    }

    // ---- modal results -------------------------------------------------------------------------

    pub fn complete(&mut self, action: Pending, value: String) {
        let insts = Installations::new(&self.svc.storage);
        match action {
            Pending::RemoveAddons(ids) if value == "n" => {
                let _ = ids;
            }
            Pending::RemoveAddons(ids) => {
                let with_deps = value == "d";
                for a in self.addons.iter().filter(|a| ids.contains(&a.id)).cloned().collect::<Vec<_>>() {
                    match self.svc.remove_addon(&a, with_deps, true) {
                        Ok(()) => self.status = format!("Removed {}", a.name),
                        Err(e) => self.errors.push(format!("{}: {e:#}", a.name)),
                    }
                }
                self.selected.clear();
                self.load_addons(false);
            }
            Pending::InstallUrl => {
                let v = value.trim().to_string();
                if v.is_empty() {
                    return;
                }
                if v.contains("://") && !v.starts_with("http") {
                    self.lookup_protocol(v);
                } else {
                    self.lookup_url(v);
                }
            }
            Pending::InstallFoundUrl(sr) => {
                if value == "y" {
                    let Some(inst) = self.inst() else { return };
                    let progress = self.progress_fn();
                    self.spawn(&format!("Installing {}", sr.name), move |svc, tx| async move {
                        let file = sr.files.first().cloned();
                        let a = svc.install_search_result(&sr, &inst, file.as_ref(), progress).await?;
                        tx.send(Msg::Status(format!("Installed {}", a.name))).ok();
                        tx.send(Msg::Reload).ok();
                        Ok(())
                    });
                }
            }
            Pending::InstallProtocol(p) => {
                if value == "y" {
                    let targets: Vec<WowInstallation> = self
                        .installations
                        .iter()
                        .filter(|i| p.valid_client_types.is_empty() || p.valid_client_types.contains(&i.client_type))
                        .filter(|i| !self.svc.is_installed(&i.id, &p.search_result.provider_name, &p.search_result.external_id))
                        .cloned()
                        .collect();
                    if targets.is_empty() {
                        self.status = "Already installed in every compatible client".into();
                        return;
                    }
                    let progress = self.progress_fn();
                    self.spawn(&format!("Installing {}", p.search_result.name), move |svc, tx| async move {
                        let file = p.search_result.files.first().cloned();
                        for inst in targets {
                            svc.install_search_result(&p.search_result, &inst, file.as_ref(), progress.clone()).await?;
                            tx.send(Msg::Status(format!("Installed {} into {}", p.search_result.name, inst.display_name))).ok();
                        }
                        tx.send(Msg::Reload).ok();
                        Ok(())
                    });
                }
            }
            Pending::AddClient => {
                let path = shellexpand(value.trim());
                match resolve_user_path(std::path::Path::new(&path)).and_then(|(loc, ct)| insts.add(&loc, ct, "")) {
                    Ok(i) => self.status = format!("Added {}", i.display_name),
                    Err(e) => self.errors.push(format!("{e:#}")),
                }
                self.reload_installations();
            }
            Pending::RemoveClient(id) => {
                if value == "y" {
                    if let Err(e) = insts.remove(&id) {
                        self.errors.push(format!("{e:#}"));
                    }
                    self.reload_installations();
                    self.addons.clear();
                }
            }
            Pending::SetLabel(id) => {
                if let Some(mut i) = self.installations.iter().find(|i| i.id == id).cloned() {
                    i.label = value.trim().to_string();
                    let _ = insts.update(&i);
                    self.reload_installations();
                }
            }
            Pending::SetKey(which) => {
                let mut p = self.svc.prefs();
                let v = (!value.trim().is_empty()).then(|| value.trim().to_string());
                match which {
                    "github" => p.github_token = v,
                    "curseforge" => {
                        p.curseforge_api_key = v.clone();
                        p.provider_enabled.insert(PROVIDER_CURSEFORGE.into(), v.is_some());
                    }
                    "wago" => {
                        p.wago_api_key = v.clone();
                        p.provider_enabled.insert(PROVIDER_WAGO.into(), v.is_some());
                    }
                    _ => {}
                }
                if let Err(e) = self.svc.save_prefs(p) {
                    self.errors.push(format!("{e:#}"));
                }
                self.status = "Saved".into();
            }
            Pending::SetInterval => {
                if let Ok(n) = value.trim().parse::<u64>() {
                    let mut p = self.svc.prefs();
                    p.auto_update_interval_minutes = n.max(5);
                    let _ = self.svc.save_prefs(p);
                }
            }
            Pending::SetBackupCount => {
                if let Ok(n) = value.trim().parse::<usize>() {
                    let mut p = self.svc.prefs();
                    p.wtf_backup_count = n;
                    let _ = self.svc.save_prefs(p);
                }
            }
            Pending::Import => {
                let Some(inst) = self.inst() else { return };
                match broker::parse(&value).and_then(|p| broker::summarize(&self.svc, &inst, &p, &self.addons).map(|s| (p, s))) {
                    Ok((_, summary)) => {
                        let body: Vec<String> = summary
                            .iter()
                            .map(|e| format!("{:<9} {:<13} {} {}", format!("{:?}", e.state), e.addon.provider_name, e.addon.name, e.reason.unwrap_or("")))
                            .collect();
                        let added = summary.iter().filter(|e| e.state == broker::ImportState::Added).count();
                        self.modal = Some(Modal::Confirm {
                            text: format!("{}\n\nInstall {added} added addon(s)?", body.join("\n")),
                            action: Pending::ApplyImport(value),
                        });
                    }
                    Err(e) => self.errors.push(format!("Import failed: {e:#}")),
                }
            }
            Pending::ApplyImport(data) => {
                if value != "y" {
                    return;
                }
                let Some(inst) = self.inst() else { return };
                let installed = self.addons.clone();
                let progress = self.progress_fn();
                self.spawn("Importing", move |svc, tx| async move {
                    let payload = broker::parse(&data)?;
                    let summary = broker::summarize(&svc, &inst, &payload, &installed)?;
                    for e in summary.into_iter().filter(|e| e.state == broker::ImportState::Added) {
                        if let Err(err) = svc.install_by_id(&e.addon.provider_name, &e.addon.id, &inst, progress.clone()).await {
                            tx.send(Msg::Error(format!("{}: {err:#}", e.addon.name))).ok();
                        }
                    }
                    tx.send(Msg::Reload).ok();
                    Ok(())
                });
            }
            Pending::RestoreBackup(file) => {
                if value == "y" {
                    let Some(inst) = self.inst() else { return };
                    match wtf::apply_backup(self.svc.storage.root(), &inst, &file) {
                        Ok(()) => self.status = format!("Restored {file}"),
                        Err(e) => self.errors.push(format!("{e:#}")),
                    }
                }
            }
            Pending::DeleteBackup(file) => {
                if value == "y" {
                    let Some(inst) = self.inst() else { return };
                    if let Err(e) = wtf::delete_backup(self.svc.storage.root(), &inst, &file) {
                        self.errors.push(format!("{e:#}"));
                    }
                    self.backups = wtf::list_backups(self.svc.storage.root(), &inst);
                    self.modal = Some(Modal::Backups { idx: 0 });
                }
            }
            Pending::Channel(ids) => {
                let ch = match value.as_str() {
                    "1" => AddonChannelType::Beta,
                    "2" => AddonChannelType::Alpha,
                    _ => AddonChannelType::Stable,
                };
                for mut a in self.addons.iter().filter(|a| ids.contains(&a.id)).cloned().collect::<Vec<_>>() {
                    if let Err(e) = self.svc.set_channel(&mut a, ch) {
                        self.errors.push(format!("{e:#}"));
                    }
                }
                self.check_updates(false);
            }
            Pending::DefaultChannel(id) => {
                if let Some(mut i) = self.installations.iter().find(|i| i.id == id).cloned() {
                    i.default_addon_channel_type = match value.as_str() {
                        "1" => AddonChannelType::Beta,
                        "2" => AddonChannelType::Alpha,
                        _ => AddonChannelType::Stable,
                    };
                    let _ = insts.update(&i);
                    self.reload_installations();
                }
            }
            Pending::Provider(addon_id) => {
                let Some(a) = self.addons.iter().find(|a| a.id == addon_id).cloned() else { return };
                let Some((prov, id)) = value.split_once('|').map(|(a, b)| (a.to_string(), b.to_string())) else { return };
                let progress = self.progress_fn();
                self.spawn(&format!("Switching {} to {prov}", a.name), move |svc, tx| async move {
                    svc.change_provider(&a, &prov, &id, progress).await?;
                    tx.send(Msg::Reload).ok();
                    Ok(())
                });
            }
            Pending::Menu => self.run_menu(&value),
            Pending::NewsOpen => self.open_link(value),
            Pending::OpenUrl(url) => {
                if value == "t" {
                    if let Some(host) = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_lowercase)) {
                        let mut p = self.svc.prefs();
                        p.trusted_domains.push(host);
                        let _ = self.svc.save_prefs(p);
                    }
                }
                if value == "y" || value == "t" {
                    let _ = fsutil::open_url(&url);
                }
            }
            Pending::Language => {
                let mut p = self.svc.prefs();
                p.language = value;
                crate::i18n::apply(&p.language);
                let _ = self.svc.save_prefs(p);
            }
            Pending::EnableSymlinks => {
                if value == "y" {
                    let mut p = self.svc.prefs();
                    p.use_symlink_mode = true;
                    let _ = self.svc.save_prefs(p);
                }
            }
            Pending::Category => {
                self.category = value.parse::<usize>().ok().and_then(|i| AddonCategory_ALL.get(i).copied());
                self.query.clear();
                self.load_results();
            }
        }
    }
}

#[allow(non_upper_case_globals)]
pub const AddonCategory_ALL: [AddonCategory; 26] = {
    use AddonCategory::*;
    [
        Achievements, ActionBars, AuctionEconomy, BagsInventory, BossEncounters, BuffsDebuffs, Bundles, ChatCommunication,
        Class, Combat, Companions, DataExport, DevelopmentTools, Guild, Libraries, Mail, MapMinimap, Miscellaneous,
        Missions, Plugins, Professions, Pvp, QuestsLeveling, Roleplay, Tooltips, UnitFrames,
    ]
};

pub fn status_rank(a: &Addon) -> u8 {
    if a.warning_type.is_some() {
        0
    } else if a.needs_update() {
        1
    } else if a.is_ignored {
        3
    } else {
        2
    }
}

fn shellexpand(p: &str) -> String {
    let p = p.trim_matches(['"', '\'']);
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| p.to_string()),
        None => p.replace("\\ ", " "),
    }
}

/// This process's memory and CPU, the terminal counterpart of upstream's process metrics table.
pub fn process_metrics() -> String {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps").args(["-o", "rss=,%cpu=,etime=", "-p", &pid]).output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let mut it = s.split_whitespace();
            let rss_kb: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let cpu = it.next().unwrap_or("?").to_string();
            let up = it.next().unwrap_or("?").to_string();
            format!("pid {pid} · memory {:.1} MB · cpu {cpu}% · uptime {up}", rss_kb / 1024.0)
        }
        Err(_) => format!("pid {pid}"),
    }
}
