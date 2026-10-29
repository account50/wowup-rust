// WowUp.rs - World of Warcraft addon manager
// SPDX-License-Identifier: GPL-3.0-or-later
//! Terminal UI. `wowup-tui [wowup://… | curseforge://… | https://…]` opens the app and, when given a
//! link, offers to install it (the protocol-handler flow).

mod app;
mod i18n;
mod ui;
mod window;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use wowup_core::addons::{AddonService, InstallState};
use wowup_core::installations::Installations;
use wowup_core::storage::Storage;
use wowup_core::{fsutil, wtf};

use app::{AddonCategory_ALL, App, Modal, Msg, OptionsTab, Pending, Tab};

/// Upstream WowUp commit this build has feature parity with (maintained by `scripts/sync-upstream.sh`).
pub const PARITY_COMMIT: &str = include_str!("../../../upstream/PARITY");

/// Desktop notification (macOS: Notification Center via osascript).
pub fn notify(title: &str, body: &str) {
    wowup_core::autostart::notify(title, body);
}

pub fn copy_to_clipboard(s: &str) -> bool {
    use std::io::Write;
    let cmd = if cfg!(target_os = "macos") { "pbcopy" } else if cfg!(windows) { "clip" } else { "xclip" };
    let mut c = std::process::Command::new(cmd);
    if cmd == "xclip" {
        c.args(["-selection", "clipboard"]);
    }
    let Ok(mut child) = c.stdin(std::process::Stdio::piped()).spawn() else { return false };
    let ok = child.stdin.as_mut().map(|i| i.write_all(s.as_bytes()).is_ok()).unwrap_or(false);
    ok && child.wait().map(|s| s.success()).unwrap_or(false)
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage = std::sync::Arc::new(Storage::open(Storage::default_root())?);
    let log_dir = storage.log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("wowup-tui.log");
    // Keep recent history (upstream keeps its last log files): start over once the log passes 2 MB.
    if std::fs::metadata(&log_path).map(|m| m.len() > 2 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&log_path, log_dir.join("wowup-tui.old.log"));
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("WOWUP_LOG").unwrap_or_else(|_| "info".into()))
        .with_writer(std::sync::Mutex::new(log))
        .with_ansi(false)
        .init();

    tracing::info!("WowUp.rs {} starting (parity {})", env!("CARGO_PKG_VERSION"), PARITY_COMMIT.trim());
    let svc = AddonService::new(storage)?;
    let lang = svc.prefs().language;
    i18n::apply(&lang);
    svc.cleanup_downloads();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(svc, tx);
    app.window = window::TerminalWindow::find();
    if let Some(w) = &app.window {
        let p = app.svc.prefs();
        // Font first: Terminal keeps rows and columns when the font changes, which would resize the window.
        if let (Some(base), true) = (p.base_font_size, (p.zoom_factor - 1.0).abs() > 1e-6) {
            w.set_font_size(base * p.zoom_factor);
        }
        if let Some(b) = p.window_bounds {
            w.set_bounds(b);
        }
    }
    show_patch_notes(&mut app);
    if !app.installations.is_empty() {
        app.load_addons(app.svc.prefs().rescan_on_start);
        app.check_updates(true);
    }
    if let Some(arg) = std::env::args().nth(1) {
        if arg.contains("://") {
            if arg.starts_with("http") {
                app.lookup_url(arg);
            } else {
                app.lookup_protocol(arg);
            }
        }
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    let res = run(&mut term, &mut app, &mut rx).await;
    if let Some(b) = app.window.as_ref().and_then(|w| w.bounds()) {
        let mut p = app.svc.prefs();
        p.window_bounds = Some(b);
        let _ = app.svc.save_prefs(p);
    }
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    res
}

async fn run(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
) -> Result<()> {
    loop {
        term.draw(|f| ui::draw(f, app))?;
        while let Ok(m) = rx.try_recv() {
            handle_msg(app, m);
        }
        // Hardware acceleration off: redraw at a low-power rate (upstream disables GPU rendering).
        let tick = if app.svc.prefs().use_hardware_acceleration { 100 } else { 500 };
        if event::poll(Duration::from_millis(tick))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    handle_key(app, k);
                }
            }
        }
        app.auto_update_tick();
        update_title(app);
        if app.quit {
            return Ok(());
        }
    }
}

fn handle_msg(app: &mut App, m: Msg) {
    match m {
        Msg::Addons(inst_id, addons) => {
            if app.inst().is_some_and(|i| i.id == inst_id) {
                app.addons = addons;
                app.selected.retain(|id| app.addons.iter().any(|a| &a.id == id));
            }
        }
        Msg::Results(r, errs) => {
            app.results = r;
            app.errors.extend(errs);
        }
        Msg::Progress(id, name, s, p) => {
            if matches!(s, InstallState::Complete) {
                app.progress.remove(&id);
            } else {
                app.progress.insert(id, (name, s, p));
            }
            if matches!(s, InstallState::Error) {
                app.progress.retain(|_, v| v.1 != InstallState::Error);
            }
        }
        Msg::Status(s) => {
            if s.is_empty() {
                app.busy = app.busy.saturating_sub(1);
                if app.busy == 0 && app.status.ends_with('…') {
                    app.status = "Ready".into();
                }
            } else {
                app.status = s;
            }
        }
        Msg::Error(e) => {
            tracing::warn!("{e}");
            app.errors.push(e);
        }
        Msg::Detail(desc, changelog, deps) => {
            if let Some(Modal::Detail(d)) = app.modal.as_mut() {
                if !desc.is_empty() {
                    d.description = desc;
                }
                if !changelog.is_empty() {
                    d.changelog = changelog;
                }
                if !deps.is_empty() {
                    d.deps = deps;
                }
            }
        }
        Msg::Backups(b) => {
            app.backups = b;
            if matches!(app.modal, Some(Modal::Backups { .. })) {
                app.modal = Some(Modal::Backups { idx: 0 });
            }
        }
        Msg::Wtf(n) => app.wtf_nodes = n,
        Msg::News(items) => {
            if items.is_empty() {
                app.modal = Some(Modal::Text {
                    title: i18n::t("PAGES.HOME.NEWS_TAB_TITLE"),
                    body: "No news right now (the Warcraft Tavern feed returned no items).".into(),
                    scroll: 0,
                });
            } else {
                let rows = items
                    .iter()
                    .map(|n| {
                        let when = n.published_at.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
                        (format!("{when}  {}  — {}", n.title, n.published_by), n.link.clone())
                    })
                    .collect();
                app.modal = Some(Modal::Pick { title: format!("{} — ⏎ open", i18n::t("PAGES.HOME.NEWS_TAB_TITLE")), items: rows, idx: 0, action: Pending::NewsOpen });
            }
        }
        Msg::UrlResult(sr) => {
            let installed = app.inst().is_some_and(|i| app.svc.is_installed(&i.id, &sr.provider_name, &sr.external_id));
            let file = sr.files.first();
            let text = format!(
                "{} by {}\n{} · {}\nVersion {}{}\n\n{}",
                sr.name,
                sr.author,
                sr.provider_name,
                sr.external_url,
                file.map(|f| f.version.clone()).unwrap_or_default(),
                sr.download_count.map(|d| format!(" · {d} downloads")).unwrap_or_default(),
                if installed { "Already installed." } else { "Install it?" }
            );
            if !installed {
                app.modal = Some(Modal::Confirm { text, action: Pending::InstallFoundUrl(Box::new(sr)) });
            } else {
                app.modal = Some(Modal::Text { title: "Install from URL".into(), body: text, scroll: 0 });
            }
        }
        Msg::Protocol(p) => {
            let clients: Vec<String> = app
                .installations
                .iter()
                .filter(|i| p.valid_client_types.is_empty() || p.valid_client_types.contains(&i.client_type))
                .map(|i| i.display_name.clone())
                .collect();
            let text = format!(
                "{} by {} ({})\n\nInstall into: {}",
                p.search_result.name,
                p.search_result.author,
                p.search_result.provider_name,
                if clients.is_empty() { "no compatible client".into() } else { clients.join(", ") }
            );
            app.modal = Some(Modal::Confirm { text, action: Pending::InstallProtocol(Box::new(p)) });
        }
        Msg::Reload => {
            app.reload_installations();
            app.load_addons(false);
        }
    }
}

fn move_cursor(c: &mut usize, len: usize, delta: i64) {
    if len == 0 {
        *c = 0;
        return;
    }
    *c = (*c as i64 + delta).clamp(0, len as i64 - 1) as usize;
}

fn nav_delta(k: &KeyEvent) -> Option<i64> {
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::PageUp => Some(-15),
        KeyCode::PageDown => Some(15),
        KeyCode::Home => Some(-100_000),
        KeyCode::End => Some(100_000),
        _ => None,
    }
}

fn handle_key(app: &mut App, k: KeyEvent) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if app.modal.is_some() {
        handle_modal_key(app, k);
        return;
    }
    if app.filtering || app.searching {
        let buf = if app.filtering { &mut app.filter } else { &mut app.query };
        match k.code {
            KeyCode::Esc => {
                buf.clear();
                app.filtering = false;
                if app.searching {
                    app.searching = false;
                    app.load_results();
                }
            }
            KeyCode::Enter => {
                if app.searching {
                    app.searching = false;
                    app.category = None;
                    app.load_results();
                }
                app.filtering = false;
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
        app.addon_cursor = 0;
        return;
    }

    // Global keys.
    match k.code {
        KeyCode::Char('q') => {
            app.quit = true;
            return;
        }
        KeyCode::Char('?') => {
            app.modal = Some(Modal::Help);
            return;
        }
        KeyCode::Char('m') => {
            app.open_menu();
            return;
        }
        KeyCode::Char('+') => {
            app.zoom(1);
            return;
        }
        KeyCode::Char('-') => {
            app.zoom(-1);
            return;
        }
        KeyCode::Char('=') => {
            app.zoom(0);
            return;
        }
        KeyCode::Char('E') => {
            app.modal = Some(Modal::Text { title: "Errors".into(), body: app.errors.join("\n"), scroll: 0 });
            app.errors.clear();
            return;
        }
        KeyCode::Char('1') => {
            app.tab = Tab::MyAddons;
            return;
        }
        KeyCode::Char('2') => {
            app.tab = Tab::GetAddons;
            if app.results.is_empty() {
                app.load_results();
            }
            return;
        }
        KeyCode::Char('3') => {
            app.tab = Tab::Options;
            return;
        }
        KeyCode::Tab => {
            app.tab = match app.tab {
                Tab::MyAddons => Tab::GetAddons,
                Tab::GetAddons => Tab::Options,
                Tab::Options => Tab::MyAddons,
            };
            if app.tab == Tab::GetAddons && app.results.is_empty() {
                app.load_results();
            }
            return;
        }
        KeyCode::Char('[') | KeyCode::Char(']') if app.tab != Tab::Options => {
            if app.installations.is_empty() {
                return;
            }
            let n = app.installations.len();
            app.inst_idx = if k.code == KeyCode::Char(']') { (app.inst_idx + 1) % n } else { (app.inst_idx + n - 1) % n };
            if let Some(i) = app.inst() {
                let _ = Installations::new(&app.svc.storage).set_selected(&i.id);
            }
            app.addons.clear();
            app.selected.clear();
            app.addon_cursor = 0;
            app.load_addons(false);
            if app.tab == Tab::GetAddons {
                app.load_results();
            }
            return;
        }
        _ => {}
    }

    match app.tab {
        Tab::MyAddons => my_addons_key(app, k),
        Tab::GetAddons => get_addons_key(app, k),
        Tab::Options => options_key(app, k),
    }
}

fn my_addons_key(app: &mut App, k: KeyEvent) {
    let len = app.visible_addons().len();
    if let Some(d) = nav_delta(&k) {
        move_cursor(&mut app.addon_cursor, len, d);
        return;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Char('a') if ctrl => {
            app.selected = app.visible_addons().iter().map(|a| a.id.clone()).collect();
        }
        KeyCode::Char('u') if ctrl => app.update_all_clients(),
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Esc => {
            app.selected.clear();
            app.filter.clear();
        }
        KeyCode::Char(' ') => {
            if let Some(a) = app.visible_addons().get(app.addon_cursor).map(|a| a.id.clone()) {
                if !app.selected.remove(&a) {
                    app.selected.insert(a);
                }
                move_cursor(&mut app.addon_cursor, len, 1);
            }
        }
        KeyCode::Char('u') => app.update_targets(false),
        KeyCode::Char('A') => app.update_targets(true),
        KeyCode::Char('s') => app.check_updates(false),
        KeyCode::Char('r') => app.load_addons(true),
        KeyCode::Char('i') => app.toggle_ignore(),
        KeyCode::Char('a') => app.toggle_auto_update(),
        KeyCode::Char('n') => app.toggle_notifications(),
        KeyCode::Char('c') => app.open_channel_picker(),
        KeyCode::Char('p') => app.open_provider_picker(),
        KeyCode::Char('x') | KeyCode::Delete => app.ask_remove(),
        KeyCode::Char('R') => app.reinstall(),
        KeyCode::Char('o') => app.open_addon_folder(),
        KeyCode::Char('w') => app.open_addon_web(),
        KeyCode::Enter | KeyCode::Char('d') => app.open_detail_for_addon(),
        KeyCode::Char('e') => app.export(),
        KeyCode::Char('I') => {
            app.modal = Some(Modal::Input { title: "Paste a WowUp import string".into(), value: String::new(), action: Pending::Import, secret: false })
        }
        KeyCode::Char('b') => app.open_backups(),
        KeyCode::Char('<') => app.set_sort(((app.sort.0 + app::MY_ADDON_COLUMNS.len() - 1) % app::MY_ADDON_COLUMNS.len(), app.sort.1)),
        KeyCode::Char('>') => app.set_sort(((app.sort.0 + 1) % app::MY_ADDON_COLUMNS.len(), app.sort.1)),
        KeyCode::Char(',') => app.set_sort((app.sort.0, !app.sort.1)),
        _ => {}
    }
}

fn get_addons_key(app: &mut App, k: KeyEvent) {
    if let Some(d) = nav_delta(&k) {
        move_cursor(&mut app.result_cursor, app.results.len(), d);
        return;
    }
    match k.code {
        KeyCode::Char('/') => {
            app.searching = true;
            app.query.clear();
        }
        KeyCode::Char('s') => app.load_results(),
        KeyCode::Char('x') => {
            app.category = None;
            app.query.clear();
            app.load_results();
        }
        KeyCode::Char('g') => {
            app.modal = Some(Modal::Pick {
                title: i18n::t("PAGES.GET_ADDONS.ADDON_CATEGORIES_MENU_TITLE"),
                items: AddonCategory_ALL.iter().enumerate().map(|(i, c)| (ui::category_text(*c), i.to_string())).collect(),
                idx: 0,
                action: Pending::Category,
            });
        }
        KeyCode::Char('U') => {
            app.modal = Some(Modal::Input {
                title: "Install from URL (WowInterface, GitHub, CurseForge, Wago, .zip) or wowup:// link".into(),
                value: String::new(),
                action: Pending::InstallUrl,
                secret: false,
            })
        }
        KeyCode::Enter => app.open_detail_for_result(),
        KeyCode::Char('i') => {
            if let Some(sr) = app.results.get(app.result_cursor).cloned() {
                app.install_result(sr);
            }
        }
        _ => {}
    }
}

fn options_key(app: &mut App, k: KeyEvent) {
    let rows = ui::option_rows(app).len();
    if let Some(d) = nav_delta(&k) {
        if !matches!(k.code, KeyCode::Char('k') | KeyCode::Char('j')) || app.options_tab != OptionsTab::Clients {
            move_cursor(&mut app.option_cursor, rows, d);
            return;
        }
    }
    match k.code {
        KeyCode::Char('[') | KeyCode::Char(']') | KeyCode::Left | KeyCode::Right => {
            let all = OptionsTab::ALL;
            let i = all.iter().position(|o| *o == app.options_tab).unwrap_or(0);
            let n = all.len();
            let fwd = matches!(k.code, KeyCode::Char(']') | KeyCode::Right);
            app.options_tab = all[if fwd { (i + 1) % n } else { (i + n - 1) % n }];
            app.option_cursor = 0;
            if app.options_tab == OptionsTab::WtfExplorer {
                app.load_wtf();
            }
            return;
        }
        _ => {}
    }
    match app.options_tab {
        OptionsTab::Clients => clients_key(app, k),
        OptionsTab::Providers => providers_key(app, k),
        OptionsTab::Application => application_key(app, k),
        OptionsTab::WtfExplorer => match k.code {
            KeyCode::Char('r') => app.load_wtf(),
            KeyCode::Char('b') => app.open_backups(),
            KeyCode::Char('o') | KeyCode::Enter => {
                if let Some(n) = app.wtf_nodes.get(app.option_cursor) {
                    let p = if n.is_dir { n.path.clone() } else { n.path.parent().map(|p| p.to_path_buf()).unwrap_or_default() };
                    let _ = fsutil::open_in_file_manager(&p);
                }
            }
            _ => {}
        },
        OptionsTab::Debug => {
            if k.code == KeyCode::Enter {
                match app.option_cursor {
                    0 => {
                        let _ = fsutil::open_in_file_manager(&app.svc.storage.log_dir());
                    }
                    1 => {
                        let _ = fsutil::open_in_file_manager(app.svc.storage.root());
                    }
                    2 => app.dump_debug_data(),
                    3 => app.view_log(),
                    _ => app.status = app::process_metrics(),
                }
            }
        }
        OptionsTab::About => {
            if k.code == KeyCode::Enter && app.option_cursor == 2 {
                app.open_link("https://github.com/WowUp/WowUp".into());
            }
        }
    }
}

fn clients_key(app: &mut App, k: KeyEvent) {
    let inst = app.installations.get(app.option_cursor).cloned();
    let insts = Installations::new(&app.svc.storage);
    match (k.code, inst) {
        (KeyCode::Char('d'), _) => app.detect_clients(),
        (KeyCode::Char('a'), _) => {
            app.modal = Some(Modal::Input {
                title: "Path to a WoW client folder, game .app/.exe, or the World of Warcraft folder".into(),
                value: String::new(),
                action: Pending::AddClient,
                secret: false,
            })
        }
        (KeyCode::Enter, Some(i)) => {
            let _ = insts.set_selected(&i.id);
            app.reload_installations();
            app.addons.clear();
            app.load_addons(false);
            app.status = format!("Selected {}", i.display_name);
        }
        (KeyCode::Char('e'), Some(i)) => {
            app.modal = Some(Modal::Input { title: "Label".into(), value: i.label.clone(), action: Pending::SetLabel(i.id), secret: false })
        }
        (KeyCode::Char('c'), Some(i)) => {
            app.modal = Some(Modal::Pick {
                title: "Default addon channel".into(),
                items: crate::ui::channel_items(),
                idx: i.default_addon_channel_type as usize,
                action: Pending::DefaultChannel(i.id),
            })
        }
        (KeyCode::Char('u'), Some(mut i)) => {
            i.default_auto_update = !i.default_auto_update;
            let _ = insts.update(&i);
            let _ = app.svc.apply_installation_auto_update(&i);
            app.reload_installations();
            app.load_addons(false);
        }
        (KeyCode::Char('K'), Some(i)) => {
            let _ = insts.reorder(&i.id, -1);
            app.reload_installations();
            move_cursor(&mut app.option_cursor, app.installations.len(), -1);
        }
        (KeyCode::Char('J'), Some(i)) => {
            let _ = insts.reorder(&i.id, 1);
            app.reload_installations();
            move_cursor(&mut app.option_cursor, app.installations.len(), 1);
        }
        (KeyCode::Char('o'), Some(i)) => {
            let _ = fsutil::open_in_file_manager(std::path::Path::new(&i.location));
        }
        (KeyCode::Char('x'), Some(i)) => {
            app.modal = Some(Modal::Confirm {
                text: format!("Remove {} from WowUp.rs?\nAddon files on disk are not deleted.", i.display_name),
                action: Pending::RemoveClient(i.id),
            })
        }
        _ => {}
    }
}

fn providers_key(app: &mut App, k: KeyEvent) {
    if k.code != KeyCode::Enter {
        return;
    }
    let providers = app.svc.all_providers();
    let n = providers.len();
    let secret_input = |title: &str, which: &'static str| Modal::Input { title: title.into(), value: String::new(), action: Pending::SetKey(which), secret: true };
    match app.option_cursor {
        i if i < n => {
            let p = &providers[i];
            if !p.is_configured() {
                app.modal = Some(match p.name() {
                    "Curse" => secret_input("CurseForge API key", "curseforge"),
                    _ => secret_input("Wago API key", "wago"),
                });
                return;
            }
            let mut prefs = app.svc.prefs();
            let cur = prefs.is_provider_enabled(p.name());
            prefs.provider_enabled.insert(p.name().to_string(), !cur);
            let _ = app.svc.save_prefs(prefs);
        }
        i if i == n => app.modal = Some(secret_input("GitHub personal access token (empty to clear)", "github")),
        i if i == n + 1 => app.modal = Some(secret_input("CurseForge API key (empty to clear)", "curseforge")),
        _ => app.modal = Some(secret_input("Wago API key (empty to clear)", "wago")),
    }
}

fn application_key(app: &mut App, k: KeyEvent) {
    if k.code != KeyCode::Enter {
        return;
    }
    let mut p = app.svc.prefs();
    let toggle_col = |p: &mut wowup_core::preferences::Preferences, c: &str| {
        if let Some(i) = p.hidden_columns.iter().position(|x| x == c) {
            p.hidden_columns.remove(i);
        } else {
            p.hidden_columns.push(c.into());
        }
    };
    let mut login_changed = false;
    match app.option_cursor {
        0 => {
            let mut items = vec![(format!("System ({})", i18n::system_language()), String::new())];
            items.extend(i18n::LANGUAGES.iter().map(|(code, name, _)| (name.to_string(), code.to_string())));
            let idx = items.iter().position(|(_, c)| *c == p.language).unwrap_or(0);
            app.modal = Some(Modal::Pick { title: i18n::t("PAGES.OPTIONS.APPLICATION.SET_LANGUAGE_LABEL"), items, idx, action: Pending::Language });
            return;
        }
        1 => {
            let i = ui::THEMES.iter().position(|t| *t == p.theme).unwrap_or(0);
            p.theme = ui::THEMES[(i + 1) % ui::THEMES.len()].into();
        }
        2 => p.enable_system_notifications = !p.enable_system_notifications,
        3 => {
            app.modal = Some(Modal::Input {
                title: "Auto update interval in minutes (min 5)".into(),
                value: p.auto_update_interval_minutes.to_string(),
                action: Pending::SetInterval,
                secret: false,
            });
            return;
        }
        4 => {
            p.start_with_system = !p.start_with_system;
            login_changed = true;
        }
        5 => {
            if !p.start_with_system {
                app.status = "Turn on launch with system first".into();
                return;
            }
            p.start_minimized = !p.start_minimized;
            login_changed = true;
        }
        6 => p.rescan_on_start = !p.rescan_on_start,
        7 => {
            if p.use_symlink_mode {
                p.use_symlink_mode = false;
            } else {
                app.modal = Some(Modal::Confirm {
                    text: format!(
                        "{}\n\n{}",
                        i18n::t("PAGES.OPTIONS.APPLICATION.USE_SYMLINK_SUPPORT_CONFIRMATION_LABEL"),
                        i18n::t("PAGES.OPTIONS.APPLICATION.USE_SYMLINK_SUPPORT_CONFIRMATION_DESCRIPTION")
                    ),
                    action: Pending::EnableSymlinks,
                });
                return;
            }
        }
        8 => {
            p.disable_ipv6 = !p.disable_ipv6;
            app.status = "Restart WowUp.rs to apply the network setting".into();
        }
        9 => p.keep_detail_tab = !p.keep_detail_tab,
        10 => p.enable_app_badge = !p.enable_app_badge,
        11 => p.use_hardware_acceleration = !p.use_hardware_acceleration,
        12 => {
            app.modal = Some(Modal::Input {
                title: "WTF backups to keep per client (0 = keep all)".into(),
                value: p.wtf_backup_count.to_string(),
                action: Pending::SetBackupCount,
                secret: false,
            });
            return;
        }
        13 => toggle_col(&mut p, "Released"),
        14 => toggle_col(&mut p, "Game"),
        15 => toggle_col(&mut p, "Author"),
        _ => {
            let _ = fsutil::open_in_file_manager(app.svc.storage.root());
            return;
        }
    }
    if login_changed {
        let cli = std::env::current_exe()
            .and_then(|e| e.canonicalize())
            .map(|e| e.with_file_name("wowup"))
            .unwrap_or_else(|_| "wowup".into());
        let log = app.svc.storage.log_dir().join("login-agent.log");
        match wowup_core::autostart::apply(p.start_with_system, p.start_minimized, p.auto_update_interval_minutes, &cli, &log) {
            Ok(()) => app.status = "Login item updated".into(),
            Err(e) => app.errors.push(format!("{e:#}")),
        }
    }
    let _ = app.svc.save_prefs(p);
}

/// Remember the details tab when "keep last opened tab" is on.
fn remember_tab(app: &App, tab: usize) {
    let mut p = app.svc.prefs();
    if p.keep_detail_tab {
        p.last_detail_tab = tab;
        let _ = app.svc.save_prefs(p);
    }
}

/// Upstream's app badge: the update count, shown in the terminal window title.
fn update_title(app: &mut App) {
    let n = app.addons.iter().filter(|a| a.needs_update()).count();
    let title = if app.svc.prefs().enable_app_badge && n > 0 { format!("WowUp.rs ({n})") } else { "WowUp.rs".into() };
    if title != app.last_title {
        let _ = execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title));
        app.last_title = title;
    }
}

/// Patch notes once per new version (`update_notes_popup_version`).
fn show_patch_notes(app: &mut App) {
    let version = env!("CARGO_PKG_VERSION");
    let mut p = app.svc.prefs();
    if p.last_version_seen.as_deref() == Some(version) {
        return;
    }
    let first_run = p.last_version_seen.is_none();
    p.last_version_seen = Some(version.into());
    let _ = app.svc.save_prefs(p);
    if !first_run {
        app.modal = Some(Modal::Text { title: format!("What's new in {version}"), body: CHANGELOG.into(), scroll: 0 });
    }
}

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

fn handle_modal_key(app: &mut App, k: KeyEvent) {
    let Some(modal) = app.modal.take() else { return };
    match modal {
        Modal::Confirm { text, action } => match k.code {
            KeyCode::Char('y') | KeyCode::Enter => app.complete(action, "y".into()),
            KeyCode::Char('t') if matches!(action, Pending::OpenUrl(_)) => app.complete(action, "t".into()),
            KeyCode::Char('d') if matches!(action, Pending::RemoveAddons(_)) => app.complete(action, "d".into()),
            KeyCode::Char('n') | KeyCode::Esc => {}
            _ => app.modal = Some(Modal::Confirm { text, action }),
        },
        Modal::Input { title, mut value, action, secret } => match k.code {
            KeyCode::Esc => {}
            KeyCode::Enter => app.complete(action, value),
            KeyCode::Backspace => {
                value.pop();
                app.modal = Some(Modal::Input { title, value, action, secret });
            }
            KeyCode::Char(c) => {
                value.push(c);
                app.modal = Some(Modal::Input { title, value, action, secret });
            }
            _ => app.modal = Some(Modal::Input { title, value, action, secret }),
        },
        Modal::Pick { title, items, mut idx, action } => match k.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                if let Some((_, v)) = items.get(idx) {
                    app.complete(action, v.clone());
                }
            }
            _ => {
                if let Some(d) = nav_delta(&k) {
                    move_cursor(&mut idx, items.len(), d);
                }
                app.modal = Some(Modal::Pick { title, items, idx, action });
            }
        },
        Modal::Text { title, body, mut scroll } => match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {}
            _ => {
                if let Some(d) = nav_delta(&k) {
                    scroll = (scroll as i64 + d).clamp(0, 10_000) as u16;
                }
                app.modal = Some(Modal::Text { title, body, scroll });
            }
        },
        Modal::Help => {}
        Modal::Backups { mut idx } => {
            let file = app.backups.get(idx).map(|b| b.file_name.clone());
            match (k.code, file) {
                (KeyCode::Esc, _) | (KeyCode::Char('q'), _) => {}
                (KeyCode::Char('n'), _) => {
                    app.create_backup();
                    app.modal = Some(Modal::Backups { idx });
                }
                (KeyCode::Enter, Some(f)) => {
                    app.modal = Some(Modal::Confirm {
                        text: format!("Restore {f}?\nYour current WTF folder will be replaced (it is kept until the restore succeeds)."),
                        action: Pending::RestoreBackup(f),
                    })
                }
                (KeyCode::Char('x'), Some(f)) => {
                    app.modal = Some(Modal::Confirm { text: format!("Delete backup {f}?"), action: Pending::DeleteBackup(f) })
                }
                (KeyCode::Char('o'), _) => {
                    if let Some(inst) = app.inst() {
                        let d = wtf::backup_dir(app.svc.storage.root(), &inst);
                        let _ = std::fs::create_dir_all(&d);
                        let _ = fsutil::open_in_file_manager(&d);
                    }
                    app.modal = Some(Modal::Backups { idx });
                }
                _ => {
                    if let Some(d) = nav_delta(&k) {
                        move_cursor(&mut idx, app.backups.len(), d);
                    }
                    app.modal = Some(Modal::Backups { idx });
                }
            }
        }
        Modal::Detail(mut d) => {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => return,
                KeyCode::Char(c @ '1'..='4') => {
                    d.tab = c as usize - '1' as usize;
                    d.scroll = 0;
                    remember_tab(app, d.tab);
                }
                KeyCode::Left => {
                    d.tab = (d.tab + 3) % 4;
                    remember_tab(app, d.tab);
                }
                KeyCode::Right => {
                    d.tab = (d.tab + 1) % 4;
                    remember_tab(app, d.tab);
                }
                KeyCode::Char('w') => {
                    let url = d.addon.as_ref().and_then(|a| a.external_url.clone()).or_else(|| d.result.as_ref().map(|r| r.external_url.clone()));
                    if let Some(u) = url.filter(|u| !u.is_empty()) {
                        app.modal = Some(Modal::Detail(d));
                        app.open_link(u);
                        return;
                    }
                }
                KeyCode::Char('o') => {
                    let urls = d.addon.as_ref().map(|a| a.screenshot_urls.clone()).or_else(|| d.result.as_ref().map(|r| r.screenshot_urls.clone()));
                    if let Some(u) = urls.and_then(|u| u.into_iter().next()) {
                        app.modal = Some(Modal::Detail(d));
                        app.open_link(u);
                        return;
                    }
                }
                KeyCode::Char('i') if d.result.is_some() => {
                    let sr = d.result.clone().unwrap();
                    app.install_result(sr);
                    return;
                }
                KeyCode::Char('u') if d.addon.is_some() => {
                    let a = d.addon.clone().unwrap();
                    if a.needs_update() {
                        app.run_installs(vec![a], false);
                    }
                    return;
                }
                KeyCode::Char('x') if d.addon.is_some() => {
                    let a = d.addon.clone().unwrap();
                    app.modal = Some(Modal::Confirm { text: format!("Remove {}?", a.name), action: Pending::RemoveAddons(vec![a.id]) });
                    return;
                }
                _ => {
                    if let Some(delta) = nav_delta(&k) {
                        d.scroll = (d.scroll as i64 + delta).clamp(0, 10_000) as u16;
                    }
                }
            }
            app.modal = Some(Modal::Detail(d));
        }
    }
}
