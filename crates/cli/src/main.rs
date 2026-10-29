// WowUp.rs - World of Warcraft addon manager
// SPDX-License-Identifier: GPL-3.0-or-later
//! `wowup` command line interface. Every TUI action is also available here for scripting.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use wowup_core::addons::{AddonService, InstallState, ProgressFn};
use wowup_core::installations::{resolve_user_path, Installations};
use wowup_core::models::*;
use wowup_core::storage::Storage;
use wowup_core::{broker, wtf};

#[derive(Parser)]
#[command(name = "wowup", version, about = "World of Warcraft addon manager (GPL-3.0, no ads)")]
struct Cli {
    /// Installation to act on: id, display name, or client type (e.g. retail, classicera). Defaults to the selected one.
    #[arg(short, long, global = true)]
    client: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage WoW installations.
    Clients {
        #[command(subcommand)]
        cmd: ClientCmd,
    },
    /// List installed addons.
    List {
        /// Only addons with updates.
        #[arg(long)]
        updates: bool,
        #[arg(long)]
        json: bool,
    },
    /// Rescan the AddOns folder and match addons to providers.
    Scan,
    /// Check providers for new versions.
    Sync,
    /// Search enabled providers.
    Search { query: Vec<String> },
    /// Featured addons for the client.
    Featured,
    /// Install from a URL (WowInterface, GitHub, CurseForge, Wago, .zip), `provider:id`, or a protocol link.
    Install { target: String },
    /// Update one addon (by name/id) or all with --all.
    Update {
        addon: Option<String>,
        #[arg(long)]
        all: bool,
        /// Update across every installation.
        #[arg(long)]
        every_client: bool,
    },
    /// Reinstall an addon.
    Reinstall { addon: String },
    /// Remove an addon and its folders.
    Remove {
        addon: String,
        /// Also remove its required dependencies.
        #[arg(long)]
        with_deps: bool,
    },
    /// Ignore / unignore an addon.
    Ignore { addon: String, #[arg(long)] off: bool },
    /// Enable / disable auto update.
    AutoUpdate { addon: String, #[arg(long)] off: bool },
    /// Set release channel: stable, beta, alpha.
    Channel { addon: String, channel: String },
    /// Switch an addon to another provider it is published on.
    Provider { addon: String, provider: String },
    /// Show addon details, description and changelog.
    Info { addon: String },
    /// Run one auto-update pass (sync + update auto-update addons). Used by the login agent.
    AutoUpdatePass {
        /// Post a system notification for updated addons (per-addon notification setting applies).
        #[arg(long)]
        notify: bool,
    },
    /// Export installed addons as a WowUp share string.
    Export,
    /// Import a WowUp share string; --apply installs the added addons.
    Import { data: String, #[arg(long)] apply: bool },
    /// WTF folder backups.
    Wtf {
        #[command(subcommand)]
        cmd: WtfCmd,
    },
    /// Show or change settings, e.g. `config set github_token XYZ`.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Handle a `wowup://` or `curseforge://` link.
    Open { url: String },
    /// Rebuild and reinstall WowUp.rs from its source checkout (pulls first when it has a git remote).
    SelfUpdate,
    /// Show news (Warcraft Tavern).
    News,
}

#[derive(Subcommand)]
enum ClientCmd {
    List,
    /// Auto-detect installs from Battle.net and the default folder.
    Detect,
    /// Add a client folder, game app/exe, or WoW root folder.
    Add { path: String, #[arg(long, default_value = "")] label: String },
    Remove { id: String },
    Select { id: String },
    /// Set defaults: --channel stable|beta|alpha, --auto-update on|off, --label.
    Set {
        id: String,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        auto_update: Option<String>,
        #[arg(long)]
        label: Option<String>,
    },
}

#[derive(Subcommand)]
enum WtfCmd {
    Backup,
    List,
    Restore { file: String },
    Delete { file: String },
}

#[derive(Subcommand)]
enum ConfigCmd {
    Show,
    Set { key: String, value: String },
}

fn progress() -> ProgressFn {
    Arc::new(|a: &Addon, s: InstallState, p: u8| {
        if matches!(s, InstallState::Downloading | InstallState::Installing | InstallState::Complete | InstallState::Error) {
            eprintln!("  {:<30} {:?} {p}%", a.name, s);
        }
    })
}

fn parse_channel(s: &str) -> Result<AddonChannelType> {
    match s.to_ascii_lowercase().as_str() {
        "stable" => Ok(AddonChannelType::Stable),
        "beta" => Ok(AddonChannelType::Beta),
        "alpha" => Ok(AddonChannelType::Alpha),
        _ => bail!("channel must be stable, beta or alpha"),
    }
}

fn pick_installation(storage: &Storage, sel: Option<&str>) -> Result<WowInstallation> {
    let insts = Installations::new(storage);
    let all = insts.all()?;
    if all.is_empty() {
        bail!("no WoW installations configured; run `wowup clients detect` or `wowup clients add <path>`");
    }
    match sel {
        None => insts.selected()?.ok_or_else(|| anyhow!("no installation selected")),
        Some(s) => {
            let norm = |x: &str| x.to_lowercase().replace([' ', '_', '-'], "");
            all.iter()
                .find(|i| i.id == s || norm(&i.display_name) == norm(s))
                .or_else(|| all.iter().find(|i| norm(i.client_type.display_name()) == norm(s)))
                .cloned()
                .ok_or_else(|| anyhow!("installation '{s}' not found"))
        }
    }
}

fn find_addon(addons: &[Addon], q: &str) -> Result<Addon> {
    let ql = q.to_lowercase();
    addons
        .iter()
        .find(|a| a.id == q || a.external_id.as_deref() == Some(q))
        .or_else(|| addons.iter().find(|a| a.name.to_lowercase() == ql))
        .or_else(|| addons.iter().find(|a| a.installed_folder_list.iter().any(|f| f.to_lowercase() == ql)))
        .or_else(|| {
            let m: Vec<&Addon> = addons.iter().filter(|a| a.name.to_lowercase().contains(&ql)).collect();
            (m.len() == 1).then(|| m[0])
        })
        .cloned()
        .ok_or_else(|| anyhow!("addon '{q}' not found (or ambiguous)"))
}

fn print_results(results: &[AddonSearchResult], errs: &[String]) {
    for r in results {
        println!(
            "{:<12} {:<36} {:<20} {:>10}  {}",
            r.provider_name,
            truncate(&r.name, 36),
            truncate(&r.author, 20),
            r.download_count.map(|d| d.to_string()).unwrap_or_default(),
            if r.provider_name == PROVIDER_GITHUB || r.provider_name == PROVIDER_ZIP {
                r.external_url.clone()
            } else {
                format!("{}:{}", r.provider_name, r.external_id)
            }
        );
    }
    for e in errs {
        eprintln!("warning: {e}");
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("WOWUP_LOG"))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let storage = Arc::new(Storage::open(Storage::default_root())?);
    let svc = AddonService::new(storage.clone())?;
    let sel = cli.client.as_deref();

    match cli.cmd {
        Cmd::Clients { cmd } => {
            let insts = Installations::new(&storage);
            match cmd {
                ClientCmd::List => {
                    for i in insts.all()? {
                        println!(
                            "{} {:<22} {:<8} auto-update:{:<5} {}  [{}]",
                            if i.selected { "*" } else { " " },
                            i.display_name,
                            format!("{:?}", i.default_addon_channel_type),
                            i.default_auto_update,
                            i.location,
                            i.id
                        );
                    }
                }
                ClientCmd::Detect => {
                    let added = insts.import_detected()?;
                    println!("added {} installation(s)", added.len());
                    for i in added {
                        println!("  {} -> {}", i.display_name, i.location);
                    }
                }
                ClientCmd::Add { path, label } => {
                    let (loc, ct) = resolve_user_path(std::path::Path::new(&path))?;
                    let i = insts.add(&loc, ct, &label)?;
                    println!("added {} ({})", i.display_name, i.location);
                }
                ClientCmd::Remove { id } => {
                    let i = pick_installation(&storage, Some(&id))?;
                    insts.remove(&i.id)?;
                    println!("removed {}", i.display_name);
                }
                ClientCmd::Select { id } => {
                    let i = pick_installation(&storage, Some(&id))?;
                    insts.set_selected(&i.id)?;
                    println!("selected {}", i.display_name);
                }
                ClientCmd::Set { id, channel, auto_update, label } => {
                    let mut i = pick_installation(&storage, Some(&id))?;
                    if let Some(c) = channel {
                        i.default_addon_channel_type = parse_channel(&c)?;
                    }
                    if let Some(l) = label {
                        i.label = l;
                    }
                    let au = auto_update.map(|v| matches!(v.as_str(), "on" | "true" | "1"));
                    if let Some(au) = au {
                        i.default_auto_update = au;
                    }
                    insts.update(&i)?;
                    if au.is_some() {
                        svc.apply_installation_auto_update(&i)?;
                    }
                    println!("updated {}", i.display_name);
                }
            }
        }
        Cmd::List { updates, json } => {
            let inst = pick_installation(&storage, sel)?;
            let addons = svc.get_addons(&inst, false).await?;
            let shown: Vec<&Addon> = addons.iter().filter(|a| !updates || a.needs_update()).collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&shown)?);
            } else {
                println!("{} — {} addons", inst.display_name, shown.len());
                for a in shown {
                    let flags = format!(
                        "{}{}{}",
                        if a.is_ignored { "I" } else { " " },
                        if a.auto_update_enabled { "A" } else { " " },
                        if a.warning_type.is_some() { "!" } else { " " }
                    );
                    println!(
                        "{flags} {:<36} {:<12} {:<20} {:<20} {}",
                        truncate(&a.name, 36),
                        a.provider(),
                        truncate(a.installed_version.as_deref().unwrap_or(""), 20),
                        if a.needs_update() { truncate(a.latest_version.as_deref().unwrap_or(""), 20) } else { String::new() },
                        a.installed_folder_list.join(",")
                    );
                }
            }
        }
        Cmd::Scan => {
            let inst = pick_installation(&storage, sel)?;
            let addons = svc.rescan(&inst).await?;
            let known = addons.iter().filter(|a| !a.is_unknown()).count();
            println!("{}: {} addons ({} matched, {} unknown)", inst.display_name, addons.len(), known, addons.len() - known);
        }
        Cmd::Sync => {
            let inst = pick_installation(&storage, sel)?;
            for i in svc.sync_installation(&inst).await {
                eprintln!("warning: [{}] {}: {}", i.provider, i.addon.unwrap_or_default(), i.message);
            }
            println!("{} update(s) available", svc.updates_available(&inst.id));
        }
        Cmd::Search { query } => {
            let inst = pick_installation(&storage, sel)?;
            let (r, e) = svc.search(&query.join(" "), &inst).await;
            print_results(&r, &e);
        }
        Cmd::Featured => {
            let inst = pick_installation(&storage, sel)?;
            let (r, e) = svc.featured(&inst).await;
            print_results(&r, &e);
        }
        Cmd::Install { target } => {
            let inst = pick_installation(&storage, sel)?;
            let addon = if target.contains("://") && !target.starts_with("http") {
                let p = svc.search_protocol(&target).await?;
                if !p.valid_client_types.is_empty() && !p.valid_client_types.contains(&inst.client_type) {
                    bail!("that release is not for {}", inst.client_type.display_name());
                }
                let file = p.search_result.files.first().cloned();
                svc.install_search_result(&p.search_result, &inst, file.as_ref(), progress()).await?
            } else if target.starts_with("http") {
                let sr = svc.search_by_url(&target, &inst).await?;
                let file = sr.files.first().cloned();
                svc.install_search_result(&sr, &inst, file.as_ref(), progress()).await?
            } else {
                let (prov, id) = target.split_once(':').ok_or_else(|| anyhow!("use a URL or provider:id"))?;
                let prov = svc
                    .all_providers()
                    .into_iter()
                    .find(|p| p.name().eq_ignore_ascii_case(prov))
                    .ok_or_else(|| anyhow!("unknown provider {prov}"))?;
                svc.install_by_id(prov.name(), id, &inst, progress()).await?
            };
            println!("installed {} {} -> {}", addon.name, addon.installed_version.unwrap_or_default(), addon.installed_folder_list.join(", "));
        }
        Cmd::Update { addon, all, every_client } => {
            let insts: Vec<WowInstallation> =
                if every_client { Installations::new(&storage).all()? } else { vec![pick_installation(&storage, sel)?] };
            for inst in insts {
                svc.sync_installation(&inst).await;
                let addons = svc.get_addons(&inst, false).await?;
                let targets: Vec<Addon> = match (&addon, all || every_client) {
                    (_, true) => addons.into_iter().filter(|a| a.needs_update()).collect(),
                    (Some(q), false) => vec![find_addon(&addons, q)?],
                    (None, false) => bail!("name an addon or pass --all"),
                };
                if targets.is_empty() {
                    println!("{}: everything is up to date", inst.display_name);
                }
                for a in targets {
                    if !a.needs_update() {
                        println!("{} is up to date", a.name);
                        continue;
                    }
                    match svc.update_addon(a.clone(), progress()).await {
                        Ok(u) => println!("updated {} to {}", u.name, u.installed_version.unwrap_or_default()),
                        Err(e) => eprintln!("failed to update {}: {e:#}", a.name),
                    }
                }
            }
        }
        Cmd::Reinstall { addon } => {
            let inst = pick_installation(&storage, sel)?;
            let a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            if !svc.can_reinstall(&a) {
                bail!("{} cannot be reinstalled", a.name);
            }
            let r = svc.install_addon(a, progress(), None).await?;
            println!("reinstalled {}", r.name);
        }
        Cmd::Remove { addon, with_deps } => {
            let inst = pick_installation(&storage, sel)?;
            let a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            svc.remove_addon(&a, with_deps, true)?;
            println!("removed {} ({})", a.name, a.installed_folder_list.join(", "));
        }
        Cmd::Ignore { addon, off } => {
            let inst = pick_installation(&storage, sel)?;
            let mut a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            svc.set_ignored(&mut a, !off)?;
            println!("{} ignored: {}", a.name, a.is_ignored);
        }
        Cmd::AutoUpdate { addon, off } => {
            let inst = pick_installation(&storage, sel)?;
            let mut a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            svc.set_auto_update(&mut a, !off)?;
            println!("{} auto update: {}", a.name, a.auto_update_enabled);
        }
        Cmd::Channel { addon, channel } => {
            let inst = pick_installation(&storage, sel)?;
            let mut a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            svc.set_channel(&mut a, parse_channel(&channel)?)?;
            println!("{} channel: {:?}", a.name, a.channel_type);
        }
        Cmd::Provider { addon, provider } => {
            let inst = pick_installation(&storage, sel)?;
            let a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            let ext = a
                .external_ids
                .iter()
                .find(|e| e.provider_name.eq_ignore_ascii_case(&provider))
                .ok_or_else(|| {
                    let avail: Vec<&str> = a.external_ids.iter().map(|e| e.provider_name.as_str()).collect();
                    anyhow!("{} is not known on {provider}; available: {}", a.name, avail.join(", "))
                })?
                .clone();
            let n = svc.change_provider(&a, &ext.provider_name, &ext.id, progress()).await?;
            println!("{} now from {}", n.name, n.provider());
        }
        Cmd::Info { addon } => {
            let inst = pick_installation(&storage, sel)?;
            let a = find_addon(&svc.get_addons(&inst, false).await?, &addon)?;
            println!("{}\n  provider: {} {}", a.name, a.provider(), a.external_id.clone().unwrap_or_default());
            println!("  author:   {}", a.author.clone().unwrap_or_default());
            println!("  version:  {} (latest {})", a.installed_version.clone().unwrap_or_default(), a.latest_version.clone().unwrap_or_default());
            println!("  channel:  {:?}  ignored: {}  auto-update: {}", a.channel_type, a.is_ignored, a.auto_update_enabled);
            println!("  folders:  {}", a.installed_folder_list.join(", "));
            println!("  game:     {}", a.game_version.join(", "));
            println!("  url:      {}", a.external_url.clone().unwrap_or_default());
            if !a.external_ids.is_empty() {
                let ids: Vec<String> = a.external_ids.iter().map(|e| format!("{}:{}", e.provider_name, e.id)).collect();
                println!("  also on:  {}", ids.join(", "));
            }
            if let Some(w) = a.warning_type {
                println!("  warning:  {w:?}");
            }
            if !a.missing_dependencies.is_empty() {
                println!("  missing dependencies: {}", a.missing_dependencies.join(", "));
            }
            for d in svc.dependency_details(&a).await {
                println!("  requires: {} ({})", d.name, if d.installed { "installed" } else { "missing" });
            }
            println!("\n{}\n", svc.description(&a).await);
            let cl = svc.changelog(&a).await;
            if !cl.is_empty() {
                println!("Changelog:\n{cl}");
            }
        }
        Cmd::AutoUpdatePass { notify } => {
            let updated = svc.process_auto_updates().await?;
            let names: Vec<String> = updated.iter().filter(|a| a.auto_update_notifications_enabled).map(|a| a.name.clone()).collect();
            if notify && svc.prefs().enable_system_notifications && !names.is_empty() {
                let body = if names.iter().map(|n| n.len()).sum::<usize>() > 60 {
                    format!("{} addons were updated", names.len())
                } else {
                    names.join("\r\n")
                };
                wowup_core::autostart::notify("WowUp.rs auto update", &body);
            }
            println!("auto-updated {} addon(s)", updated.len());
            for a in updated {
                println!("  {} {}", a.name, a.installed_version.unwrap_or_default());
            }
        }
        Cmd::Export => {
            let inst = pick_installation(&storage, sel)?;
            println!("{}", broker::export(&inst, &svc.get_addons(&inst, false).await?)?);
        }
        Cmd::Import { data, apply } => {
            let inst = pick_installation(&storage, sel)?;
            let payload = broker::parse(&data)?;
            let installed = svc.get_addons(&inst, false).await?;
            let summary = broker::summarize(&svc, &inst, &payload, &installed)?;
            for e in &summary {
                println!("{:<9} {:<12} {} {}", format!("{:?}", e.state), e.addon.provider_name, e.addon.name, e.reason.unwrap_or(""));
            }
            if apply {
                for e in summary.iter().filter(|e| e.state == broker::ImportState::Added) {
                    match svc.install_by_id(&e.addon.provider_name, &e.addon.id, &inst, progress()).await {
                        Ok(a) => println!("installed {}", a.name),
                        Err(err) => eprintln!("failed {}: {err:#}", e.addon.name),
                    }
                }
            }
        }
        Cmd::Wtf { cmd } => {
            let inst = pick_installation(&storage, sel)?;
            match cmd {
                WtfCmd::Backup => println!("created {}", wtf::create_backup(storage.root(), &inst)?.display()),
                WtfCmd::List => {
                    for b in wtf::list_backups(storage.root(), &inst) {
                        println!(
                            "{:<26} {:>10} {} {}",
                            b.file_name,
                            b.size,
                            b.created_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                            b.error.unwrap_or_default()
                        );
                    }
                }
                WtfCmd::Restore { file } => {
                    wtf::apply_backup(storage.root(), &inst, &file)?;
                    println!("restored {file}");
                }
                WtfCmd::Delete { file } => {
                    wtf::delete_backup(storage.root(), &inst, &file)?;
                    println!("deleted {file}");
                }
            }
        }
        Cmd::Config { cmd } => {
            let mut p = svc.prefs();
            match cmd {
                ConfigCmd::Show => {
                    let mut v = serde_json::to_value(&p)?;
                    for k in ["curseforgeApiKey", "wagoApiKey", "githubToken"] {
                        if v[k].is_string() {
                            v[k] = "<set>".into();
                        }
                    }
                    println!("{}", serde_json::to_string_pretty(&v)?);
                    println!("data dir: {}", storage.root().display());
                }
                ConfigCmd::Set { key, value } => {
                    let opt = |v: &str| (!v.is_empty()).then(|| v.to_string());
                    let b = matches!(value.as_str(), "on" | "true" | "1" | "yes");
                    match key.as_str() {
                        "github_token" => p.github_token = opt(&value),
                        "curseforge_api_key" => p.curseforge_api_key = opt(&value),
                        "wago_api_key" => p.wago_api_key = opt(&value),
                        "auto_update_interval_minutes" => p.auto_update_interval_minutes = value.parse()?,
                        "enable_system_notifications" => p.enable_system_notifications = b,
                        "enable_wtf_backups" => p.enable_wtf_backups = b,
                        "wtf_backup_count" => p.wtf_backup_count = value.parse()?,
                        "use_symlink_mode" => p.use_symlink_mode = b,
                        "disable_ipv6" => p.disable_ipv6 = b,
                        "language" => p.language = value.clone(),
                        "enable_app_badge" => p.enable_app_badge = b,
                        "start_with_system" | "start_minimized" => {
                            if key == "start_with_system" {
                                p.start_with_system = b;
                            } else {
                                p.start_minimized = b;
                            }
                            let cli = std::env::current_exe()?.canonicalize()?;
                            wowup_core::autostart::apply(
                                p.start_with_system,
                                p.start_minimized,
                                p.auto_update_interval_minutes,
                                &cli,
                                &storage.log_dir().join("login-agent.log"),
                            )?;
                        }
                        "theme" => p.theme = value.clone(),
                        k if k.starts_with("provider.") => {
                            let name = &k["provider.".len()..];
                            let prov = svc
                                .all_providers()
                                .into_iter()
                                .find(|x| x.name().eq_ignore_ascii_case(name))
                                .ok_or_else(|| anyhow!("unknown provider {name}"))?;
                            p.provider_enabled.insert(prov.name().to_string(), b);
                        }
                        _ => bail!("unknown setting {key}"),
                    }
                    svc.save_prefs(p)?;
                    println!("saved {key}");
                }
            }
        }
        Cmd::SelfUpdate => {
            let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let script = src.join("scripts/install-macos.sh");
            if !script.exists() {
                bail!("source checkout not found at {}; clone WowUp.rs and run scripts/install-macos.sh", src.display());
            }
            // Pull only when the branch tracks an upstream; otherwise rebuild what is checked out.
            let has_upstream = std::process::Command::new("git")
                .arg("-C")
                .arg(&src)
                .args(["rev-parse", "--abbrev-ref", "@{u}"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if has_upstream {
                println!("==> git pull");
                let ok = std::process::Command::new("git").arg("-C").arg(&src).args(["pull", "--ff-only"]).status()?.success();
                if !ok {
                    bail!("git pull failed");
                }
            }
            let ok = std::process::Command::new(&script).status()?.success();
            if !ok {
                bail!("install failed");
            }
        }
        Cmd::News => {
            let items = wowup_core::news::load(&svc.net).await;
            if items.is_empty() {
                println!("No news right now.");
            }
            for n in items {
                println!("{}  {}  — {}\n    {}", n.published_at.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(), n.title, n.published_by, n.link);
            }
        }
        Cmd::Open { url } => {
            let inst = pick_installation(&storage, sel)?;
            let p = svc.search_protocol(&url).await?;
            println!("{} by {} ({})", p.search_result.name, p.search_result.author, p.search_result.provider_name);
            let file = p.search_result.files.first().cloned();
            let a = svc.install_search_result(&p.search_result, &inst, file.as_ref(), progress()).await?;
            println!("installed {} into {}", a.name, inst.display_name);
        }
    }
    Ok(())
}
