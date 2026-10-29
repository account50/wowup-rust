// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawing. Layout mirrors WowUp: tabs for My Addons / Get Addons / Options, a client selector,
//! a table, and a footer with status and key hints.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Tabs, Wrap};
use ratatui::Frame;
use wowup_core::addons::InstallState;
use wowup_core::models::*;

use crate::app::{status_rank, App, Modal, OptionsTab, Tab, MY_ADDON_COLUMNS};
use crate::i18n::{t as t_, tp as tp_};

pub struct Theme {
    pub accent: Color,
    pub dim: Color,
    pub warn: Color,
    pub ok: Color,
    pub update: Color,
}

pub fn theme(name: &str) -> Theme {
    match name {
        "alliance" => Theme { accent: Color::Rgb(70, 130, 220), dim: Color::DarkGray, warn: Color::Yellow, ok: Color::Green, update: Color::Rgb(240, 200, 60) },
        "default" => Theme { accent: Color::Rgb(150, 100, 230), dim: Color::DarkGray, warn: Color::Yellow, ok: Color::Green, update: Color::Cyan },
        _ => Theme { accent: Color::Rgb(200, 50, 50), dim: Color::DarkGray, warn: Color::Yellow, ok: Color::Green, update: Color::Rgb(240, 200, 60) },
    }
}

pub const THEMES: [&str; 3] = ["horde", "alliance", "default"];

pub fn draw(f: &mut Frame, app: &App) {
    let t = theme(&app.svc.prefs().theme);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(2)])
        .split(f.area());

    draw_header(f, app, &t, chunks[0]);
    match app.tab {
        Tab::MyAddons => draw_my_addons(f, app, &t, chunks[1]),
        Tab::GetAddons => draw_get_addons(f, app, &t, chunks[1]),
        Tab::Options => draw_options(f, app, &t, chunks[1]),
    }
    draw_footer(f, app, &t, chunks[2]);
    if let Some(m) = &app.modal {
        draw_modal(f, app, &t, m);
    }
}

fn draw_header(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(44)])
        .split(area);
    let idx = match app.tab {
        Tab::MyAddons => 0,
        Tab::GetAddons => 1,
        Tab::Options => 2,
    };
    let updates: usize = app.addons.iter().filter(|a| a.needs_update()).count();
    let my = format!("1 {}", t_("PAGES.HOME.MY_ADDONS_TAB_TITLE"));
    let my = if updates > 0 { format!("{my} ({updates})") } else { my };
    let tabs = Tabs::new(vec![my, format!("2 {}", t_("PAGES.HOME.GET_ADDONS_TAB_TITLE")), format!("3 {}", t_("PAGES.HOME.OPTIONS_TAB_TITLE"))])
        .select(idx)
        .block(Block::bordered().border_type(BorderType::Rounded).title(Span::styled(" WowUp.rs ", Style::new().fg(t.accent).bold())))
        .highlight_style(Style::new().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED));
    f.render_widget(tabs, cols[0]);
    let client = app
        .inst()
        .map(|i| format!("{}  ◂ [ ] ▸", i.display_name))
        .unwrap_or_else(|| "no client".into());
    f.render_widget(
        Paragraph::new(client).block(Block::bordered().border_type(BorderType::Rounded).title(" Client ")),
        cols[1],
    );
}

fn status_cell(app: &App, a: &Addon, t: &Theme) -> Cell<'static> {
    if let Some((_, s, p)) = app.progress.get(&a.id) {
        return Cell::from(format!("{} {p}%", state_text(*s))).style(Style::new().fg(t.accent));
    }
    match status_rank(a) {
        0 => Cell::from(format!("⚠ {}", warning_text(a.warning_type, a.provider()))).style(Style::new().fg(t.warn)),
        1 => Cell::from(format!("⬆ {}", t_("COMMON.ADDON_STATE.UPDATE"))).style(Style::new().fg(t.update).bold()),
        3 if a.is_unknown() => Cell::from(match a.ignore_reason {
            Some(AddonIgnoreReason::GitRepo) => "git repo",
            _ => "unknown",
        })
        .style(Style::new().fg(t.dim)),
        3 => Cell::from(t_("COMMON.ADDON_STATE.IGNORED")).style(Style::new().fg(t.dim)),
        _ => Cell::from(format!("✓ {}", t_("COMMON.ADDON_STATE.UPTODATE"))).style(Style::new().fg(t.ok)),
    }
}

fn warning_text(w: Option<AddonWarningType>, provider: &str) -> String {
    let key = match w {
        Some(AddonWarningType::MissingOnProvider) => "MISSING_ON_PROVIDER_TOOLTIP",
        Some(AddonWarningType::NoProviderFiles) => "NO_PROVIDER_FILES_TOOLTIP",
        Some(AddonWarningType::TocNameMismatch) => "TOC_NAME_MISMATCH_TOOLTIP",
        Some(AddonWarningType::GameVersionTocMissing) => "GAME_VERSION_TOC_MISSING_TOOLTIP",
        None => "GENERIC_TOOLTIP",
    };
    tp_(&format!("COMMON.ADDON_WARNING.{key}"), &[("providerName", provider.to_string())])
}

pub fn state_text(s: InstallState) -> String {
    let key = match s {
        InstallState::Pending => "PENDING",
        InstallState::Downloading => "DOWNLOADING",
        InstallState::BackingUp => "BACKINGUP",
        InstallState::Installing => "INSTALLING",
        InstallState::Complete => "COMPLETE",
        InstallState::Retry => "RETRY",
        InstallState::Error => "ERROR",
    };
    t_(&format!("COMMON.ADDON_STATUS.{key}"))
}

/// Display title for a My Addons column id.
pub fn column_title(id: &str) -> String {
    let key = match id {
        "Addon" => "ADDON_COLUMN_HEADER",
        "Status" => "STATUS_COLUMN_HEADER",
        "Latest" => "LATEST_VERSION_COLUMN_HEADER",
        "Released" => "RELEASED_AT_COLUMN_HEADER",
        "Game" => "GAME_VERSION_COLUMN_HEADER",
        "Provider" => "PROVIDER_COLUMN_HEADER",
        _ => "AUTHOR_COLUMN_HEADER",
    };
    t_(&format!("PAGES.MY_ADDONS.TABLE.{key}"))
}

pub fn channel_text(c: AddonChannelType) -> String {
    t_(&format!("COMMON.ENUM.ADDON_CHANNEL_TYPE.{}", format!("{c:?}").to_uppercase()))
}

/// `QuestsLeveling` -> `COMMON.ADDON_CATEGORIES.QUESTS_LEVELING`.
pub fn category_text(c: AddonCategory) -> String {
    let mut snake = String::new();
    for (i, ch) in format!("{c:?}").chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            snake.push('_');
        }
        snake.push(ch.to_ascii_uppercase());
    }
    t_(&format!("COMMON.ADDON_CATEGORIES.{}", snake.replace("P_V_P", "PVP")))
}

fn draw_my_addons(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let rows_area = if app.filtering || !app.filter.is_empty() {
        let c = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);
        f.render_widget(
            Paragraph::new(format!("{}{}", app.filter, if app.filtering { "▏" } else { "" }))
                .block(Block::bordered().border_type(BorderType::Rounded).title(" Filter (Esc to clear) ")),
            c[0],
        );
        c[1]
    } else {
        area
    };
    let vis = app.visible_addons();
    let header = Row::new(MY_ADDON_COLUMNS.iter().enumerate().map(|(i, c)| {
        let arrow = if app.sort.0 == i { if app.sort.1 { " ▲" } else { " ▼" } } else { "" };
        Cell::from(format!("{}{arrow}", column_title(c)))
    }))
    .style(Style::new().fg(t.accent).bold());
    let rows = vis.iter().map(|a| {
        let mut name = String::new();
        if app.selected.contains(&a.id) {
            name.push_str("● ");
        }
        name.push_str(&a.name);
        let mut badges = String::new();
        if a.auto_update_enabled {
            badges.push_str(" ⟳");
        }
        if a.channel_type != AddonChannelType::Stable {
            badges.push_str(&format!(" [{:?}]", a.channel_type));
        }
        if a.external_ids.len() > 1 {
            badges.push_str(" ⇄");
        }
        if a.dependencies.iter().any(|d| d.dep_type == AddonDependencyType::Required) {
            badges.push_str(" ⛓");
        }
        if !a.missing_dependencies.is_empty() {
            badges.push_str(" ✗dep");
        }
        let version = a.installed_version.clone().unwrap_or_default();
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::raw(name),
                Span::styled(badges, Style::new().fg(t.dim)),
                Span::styled(format!("  {version}"), Style::new().fg(t.dim)),
            ])),
            status_cell(app, a, t),
            Cell::from(if a.needs_update() { a.latest_version.clone().unwrap_or_default() } else { String::new() }),
            Cell::from(a.released_at.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()),
            Cell::from(game_version_for(a, app.inst().map(|i| i.client_type))),
            Cell::from(a.provider().to_string()),
            Cell::from(a.author.clone().unwrap_or_default()),
        ])
    });
    let hidden = app.svc.prefs().hidden_columns;
    let widths: Vec<Constraint> = MY_ADDON_COLUMNS
        .iter()
        .map(|c| {
            if hidden.iter().any(|h| h == c) {
                return Constraint::Length(0);
            }
            match *c {
                "Addon" => Constraint::Min(30),
                "Status" => Constraint::Length(20),
                "Latest" => Constraint::Length(16),
                "Released" => Constraint::Length(11),
                "Game" => Constraint::Length(9),
                "Provider" => Constraint::Length(13),
                _ => Constraint::Length(18),
            }
        })
        .collect();
    let title = app
        .inst()
        .map(|i| format!(
                " {} — {} · {} ",
                i.display_name,
                tp_("PAGES.MY_ADDONS.PAGE_CONTEXT_FOOTER.ADDONS_INSTALLED", &[("count", vis.len().to_string())]),
                tp_("PAGES.MY_ADDONS.ADDON_CONTEXT_MENU.ADDONS_SELECTED", &[("count", app.selected.len().to_string())])
            ))
        .unwrap_or_default();
    // Installs not yet in the list (new installs and their pending dependencies) show as extra rows.
    let pending: Vec<Row> = app
        .progress
        .iter()
        .filter(|(id, _)| !app.addons.iter().any(|a| &a.id == *id))
        .map(|(_, (name, s, p))| {
            Row::new(vec![
                Cell::from(name.clone()).style(Style::new().fg(t.dim)),
                Cell::from(format!("{} {p}%", state_text(*s))).style(Style::new().fg(t.accent)),
            ])
        })
        .collect();
    let rows = rows.chain(pending);
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().border_type(BorderType::Rounded).title(title))
        .row_highlight_style(Style::new().bg(Color::Rgb(45, 45, 60)).bold());
    let mut state = TableState::default().with_selected(Some(app.addon_cursor.min(vis.len().saturating_sub(1))));
    if vis.is_empty() {
        f.render_widget(
            Paragraph::new("No addons. Press `r` to scan, or install some from Get Addons (2).")
                .block(Block::bordered().border_type(BorderType::Rounded)),
            rows_area,
        );
    } else {
        f.render_stateful_widget(table, rows_area, &mut state);
    }
}

/// The interface version that applies to the client: one matching its major version, else the newest.
pub fn game_version_for(a: &Addon, client: Option<WowClientType>) -> String {
    let prefix = client.map(|c| c.major_version()).unwrap_or("");
    let key = |v: &String| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    a.game_version
        .iter()
        .find(|v| !prefix.is_empty() && v.starts_with(prefix))
        .or_else(|| a.game_version.iter().max_by_key(|v| key(v)))
        .cloned()
        .unwrap_or_default()
}

fn draw_get_addons(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let c = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);
    let title = match (&app.category, app.query.is_empty()) {
        (_, false) => " Search ".to_string(),
        (Some(cat), true) => format!(" {} (x) ", tp_("PAGES.GET_ADDONS.ADDON_CATEGORIES_SELECTED_TITLE", &[("category", category_text(*cat))])),
        (None, true) => " Featured — / to search, g categories, U install from URL ".to_string(),
    };
    f.render_widget(
        Paragraph::new(format!("{}{}", app.query, if app.searching { "▏" } else { "" }))
            .block(Block::bordered().border_type(BorderType::Rounded).title(title)),
        c[0],
    );
    let header = Row::new(
        ["ADDON_COLUMN_HEADER", "DOWNLOAD_COUNT_COLUMN_HEADER", "RELEASED_AT_COLUMN_HEADER", "AUTHOR_COLUMN_HEADER", "PROVIDER_COLUMN_HEADER", "STATUS_COLUMN_HEADER"]
            .map(|k| t_(&format!("PAGES.GET_ADDONS.TABLE.{k}"))),
    ).style(Style::new().fg(t.accent).bold());
    let inst_id = app.inst().map(|i| i.id).unwrap_or_default();
    let rows = app.results.iter().map(|r| {
        let installed = app.svc.is_installed(&inst_id, &r.provider_name, &r.external_id);
        let lf = r.latest_file(AddonChannelType::Alpha);
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::raw(r.name.clone()),
                Span::styled(
                    format!(
                        "  {}",
                        wowup_core::providers::html_to_text(r.summary.as_deref().unwrap_or(""))
                            .replace('\n', " ")
                            .chars()
                            .take(70)
                            .collect::<String>()
                    ),
                    Style::new().fg(t.dim),
                ),
            ])),
            Cell::from(r.download_count.map(fmt_count).unwrap_or_default()),
            Cell::from(lf.and_then(|f| f.release_date).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()),
            Cell::from(r.author.clone()),
            Cell::from(r.provider_name.clone()),
            if r.externally_blocked {
                Cell::from(t_("COMMON.ADDON_STATE.UNAVAILABLE")).style(Style::new().fg(t.dim))
            } else if installed {
                Cell::from(format!("✓ {}", t_("COMMON.ADDON_STATUS.COMPLETE"))).style(Style::new().fg(t.ok))
            } else {
                Cell::from(format!("{} ⏎", t_("COMMON.ADDON_STATE.INSTALL"))).style(Style::new().fg(t.update))
            },
        ])
    });
    let table = Table::new(
        rows,
        [Constraint::Min(30), Constraint::Length(10), Constraint::Length(11), Constraint::Length(20), Constraint::Length(13), Constraint::Length(12)],
    )
    .header(header)
    .block(Block::bordered().border_type(BorderType::Rounded).title(format!(" {} results ", app.results.len())))
    .row_highlight_style(Style::new().bg(Color::Rgb(45, 45, 60)).bold());
    let mut state = TableState::default().with_selected(Some(app.result_cursor));
    f.render_stateful_widget(table, c[1], &mut state);
}

fn fmt_count(n: u64) -> String {
    if n >= 2_000_000_000 {
        "—".into()
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn option_rows(app: &App) -> Vec<(String, String)> {
    let p = app.svc.prefs();
    let on = |b: bool| if b { "on" } else { "off" }.to_string();
    let set = |o: &Option<String>| if o.is_some() { "set" } else { "not set" }.to_string();
    match app.options_tab {
        OptionsTab::Clients => app
            .installations
            .iter()
            .map(|i| {
                (
                    format!("{}{}", if i.selected { "● " } else { "  " }, i.display_name),
                    format!("channel {:?} · auto-update {} · {}", i.default_addon_channel_type, on(i.default_auto_update), i.location),
                )
            })
            .collect(),
        OptionsTab::Providers => {
            let mut rows: Vec<(String, String)> = app
                .svc
                .all_providers()
                .iter()
                .map(|pr| {
                    let enabled = p.is_provider_enabled(pr.name());
                    let state = if !pr.is_configured() { "needs key".to_string() } else { on(enabled) };
                    (pr.name().to_string(), format!("{state}  {}", pr.note()))
                })
                .collect();
            rows.push(("GitHub token".into(), set(&p.github_token)));
            rows.push(("CurseForge API key".into(), set(&p.curseforge_api_key)));
            rows.push(("Wago API key".into(), set(&p.wago_api_key)));
            rows
        }
        OptionsTab::Application => {
            let k = |key: &str| t_(&format!("PAGES.OPTIONS.APPLICATION.{key}"));
            let lang = if p.language.is_empty() {
                format!("System ({})", crate::i18n::system_language())
            } else {
                crate::i18n::LANGUAGES.iter().find(|l| l.0 == p.language).map(|l| l.1.to_string()).unwrap_or(p.language.clone())
            };
            vec![
                (k("CURRENT_LANGUAGE_LABEL"), lang),
                (k("THEME_LABEL"), p.theme.clone()),
                (k("ENABLE_SYSTEM_NOTIFICATIONS_LABEL"), on(p.enable_system_notifications)),
                ("Auto update interval (minutes)".into(), p.auto_update_interval_minutes.to_string()),
                (k("START_WITH_SYSTEM_LABEL"), on(p.start_with_system)),
                (k("START_MINIMIZED_LABEL"), format!("{}  (background auto-updates at login)", on(p.start_minimized))),
                ("Rescan on start".into(), on(p.rescan_on_start)),
                (k("USE_SYMLINK_SUPPORT"), on(p.use_symlink_mode)),
                (k("DISABLE_IPV6_LABEL"), on(p.disable_ipv6)),
                (k("KEEP_LAST_OPENED_TAB_LABEL"), on(p.keep_detail_tab)),
                (k("ENABLE_APP_BADGE_LABEL"), on(p.enable_app_badge)),
                (k("USE_HARDWARE_ACCELERATION_LABEL"), format!("{}  (off = low-power redraw rate)", on(p.use_hardware_acceleration))),
                ("WTF backups to keep".into(), p.wtf_backup_count.to_string()),
                (format!("Hide column: {}", column_title("Released")), on(p.hidden_columns.iter().any(|c| c == "Released"))),
                (format!("Hide column: {}", column_title("Game")), on(p.hidden_columns.iter().any(|c| c == "Game"))),
                (format!("Hide column: {}", column_title("Author")), on(p.hidden_columns.iter().any(|c| c == "Author"))),
                ("Open log/config folder".into(), app.svc.storage.root().display().to_string()),
            ]
        }
        OptionsTab::WtfExplorer => app
            .wtf_nodes
            .iter()
            .map(|n| {
                let mut label = format!("{}{}{}", "  ".repeat(n.depth), if n.is_dir { "▸ " } else { "" }, n.name);
                if n.orphaned {
                    label.push_str("  (addon not installed)");
                }
                (label, fmt_size(n.size))
            })
            .collect(),
        OptionsTab::Debug => {
            let k = |key: &str| t_(&format!("PAGES.OPTIONS.DEBUG.{key}"));
            vec![
                (k("LOG_FILES_LABEL"), format!("⏎ {}  ({})", k("LOG_FILES_BUTTON"), app.svc.storage.log_dir().display())),
                (k("CONFIG_FILES_LABEL"), format!("⏎ {}  ({})", k("CONFIG_FILES_BUTTON"), app.svc.storage.root().display())),
                (k("DEBUG_DATA_LABEL"), format!("⏎ {}", k("DEBUG_DATA_BUTTON"))),
                ("Log viewer".into(), "⏎ show the latest log lines".into()),
                ("Process metrics".into(), crate::app::process_metrics()),
            ]
        }
        OptionsTab::About => vec![
            ("WowUp.rs".into(), format!("v{}", env!("CARGO_PKG_VERSION"))),
            ("License".into(), "GNU GPL v3 or later — a Rust port of WowUp".into()),
            ("Upstream".into(), "https://github.com/WowUp/WowUp".into()),
            ("Parity with upstream commit".into(), crate::PARITY_COMMIT.into()),
            ("Ads / telemetry".into(), "none — removed from this port".into()),
            ("Data folder".into(), app.svc.storage.root().display().to_string()),
        ],
    }
}

fn fmt_size(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn draw_options(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let c = Layout::horizontal([Constraint::Length(24), Constraint::Min(30)]).split(area);
    let items: Vec<ListItem> = OptionsTab::ALL.iter().map(|o| ListItem::new(o.title())).collect();
    let mut ls = ListState::default().with_selected(OptionsTab::ALL.iter().position(|o| *o == app.options_tab));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().border_type(BorderType::Rounded).title(" Options ([ ]) "))
            .highlight_style(Style::new().fg(t.accent).bold().add_modifier(Modifier::REVERSED)),
        c[0],
        &mut ls,
    );
    let rows = option_rows(app);
    let hint = match app.options_tab {
        OptionsTab::Clients => " ⏎ select · d detect · a add · e label · c channel · u auto-update · K/J move · o open · x remove ",
        OptionsTab::Providers => " ⏎ toggle provider / edit key ",
        OptionsTab::Application => " ⏎ change ",
        OptionsTab::WtfExplorer => " r refresh · o open · b backups ",
        OptionsTab::Debug => " ⏎ run ",
        OptionsTab::About => "",
    };
    let wtf = app.options_tab == OptionsTab::WtfExplorer;
    let table = Table::new(
        rows.iter().enumerate().map(|(i, (k, v))| {
            let mut style = Style::new();
            if wtf {
                if let Some(n) = app.wtf_nodes.get(i) {
                    if n.ignorable {
                        style = style.fg(t.dim);
                    } else if n.orphaned {
                        style = style.fg(Color::Red);
                    }
                }
            }
            Row::new(vec![k.clone(), v.clone()]).style(style)
        }),
        [Constraint::Percentage(40), Constraint::Percentage(60)],
    )
    .block(Block::bordered().border_type(BorderType::Rounded).title(format!(" {} ", app.options_tab.title())).title_bottom(hint))
    .row_highlight_style(Style::new().bg(Color::Rgb(45, 45, 60)).bold());
    let mut st = TableState::default().with_selected(Some(app.option_cursor.min(rows.len().saturating_sub(1))));
    f.render_stateful_widget(table, c[1], &mut st);
}

fn draw_footer(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let c = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let active: Vec<String> = app
        .progress
        .values()
        .filter(|(_, s, _)| !matches!(s, InstallState::Complete | InstallState::Error))
        .map(|(n, s, p)| format!("{n} {} {p}%", state_text(*s)))
        .collect();
    let mut status = if !active.is_empty() { active.join(" · ") } else { app.status.clone() };
    if app.busy > 0 {
        status = format!("⏳ {status}");
    }
    if let Some(e) = app.errors.last() {
        status = format!("{status}   ⚠ {e}{}", if app.errors.len() > 1 { format!(" (+{} more, E to view)", app.errors.len() - 1) } else { String::new() });
    }
    f.render_widget(Paragraph::new(status).style(Style::new().fg(if app.errors.is_empty() { Color::Reset } else { t.warn })), c[0]);
    let keys = match app.tab {
        Tab::MyAddons => "u update · A update all · s check · r rescan · ␣ select · i ignore · a auto · c channel · p provider · ⏎ details · x remove · R reinstall · e export · I import · b WTF · ? help · q quit",
        Tab::GetAddons => "/ search · g category · x reset · ⏎ details · i install · U install URL/link · s refresh · ? help · q quit",
        Tab::Options => "[ ] section · ↑↓ select · ⏎ change · ? help · q quit",
    };
    f.render_widget(Paragraph::new(keys).style(Style::new().fg(t.dim)), c[1]);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_modal(f: &mut Frame, app: &App, t: &Theme, m: &Modal) {
    let block = |title: String| Block::bordered().border_type(BorderType::Double).border_style(Style::new().fg(t.accent)).title(title);
    match m {
        Modal::Confirm { text, .. } => {
            let h = (text.lines().count() as u16 + 4).min(30);
            let r = centered(f.area(), 80, h);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(format!("{text}\n\n[y] yes   [n] no")).wrap(Wrap { trim: false }).block(block(" Confirm ".into())),
                r,
            );
        }
        Modal::Input { title, value, secret, .. } => {
            let r = centered(f.area(), 90, 5);
            f.render_widget(Clear, r);
            let shown = if *secret { "•".repeat(value.chars().count()) } else { value.clone() };
            f.render_widget(Paragraph::new(format!("{shown}▏\n⏎ ok · Esc cancel")).block(block(format!(" {title} "))), r);
        }
        Modal::Pick { title, items, idx, .. } => {
            let r = centered(f.area(), 60, items.len() as u16 + 2);
            f.render_widget(Clear, r);
            let mut ls = ListState::default().with_selected(Some(*idx));
            f.render_stateful_widget(
                List::new(items.iter().map(|(l, _)| ListItem::new(l.clone())))
                    .block(block(format!(" {title} ")))
                    .highlight_style(Style::new().fg(t.accent).bold().add_modifier(Modifier::REVERSED)),
                r,
                &mut ls,
            );
        }
        Modal::Text { title, body, scroll } => {
            let r = centered(f.area(), 100, 30);
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(body.clone()).wrap(Wrap { trim: false }).scroll((*scroll, 0)).block(block(format!(" {title} — Esc to close "))), r);
        }
        Modal::Backups { idx } => {
            let r = centered(f.area(), 90, 20);
            f.render_widget(Clear, r);
            let items: Vec<ListItem> = app
                .backups
                .iter()
                .map(|b| {
                    ListItem::new(format!(
                        "{}  {}  {}{}",
                        b.created_at.map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
                        fmt_size(b.size),
                        b.file_name,
                        b.error.as_ref().map(|e| format!("  ⚠ {e}")).unwrap_or_default()
                    ))
                })
                .collect();
            let mut ls = ListState::default().with_selected(Some(*idx));
            f.render_stateful_widget(
                List::new(items)
                    .block(block(format!(" WTF Backups ({}) — n new · ⏎ restore · x delete · o folder · Esc ", app.backups.len())))
                    .highlight_style(Style::new().fg(t.accent).bold().add_modifier(Modifier::REVERSED)),
                r,
                &mut ls,
            );
        }
        Modal::Help => {
            let r = centered(f.area(), 90, 32);
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(HELP).block(block(" Help ".into())), r);
        }
        Modal::Detail(d) => {
            let r = centered(f.area(), 110, 34);
            f.render_widget(Clear, r);
            let (name, author, provider, ext_id, url) = match (&d.addon, &d.result) {
                (Some(a), _) => (a.name.clone(), a.author.clone().unwrap_or_default(), a.provider().to_string(), a.external_id.clone().unwrap_or_default(), a.external_url.clone().unwrap_or_default()),
                (_, Some(s)) => (s.name.clone(), s.author.clone(), s.provider_name.clone(), s.external_id.clone(), s.external_url.clone()),
                _ => Default::default(),
            };
            let tabs = [
                t_("DIALOGS.ADDON_DETAILS.DESCRIPTION_TAB"),
                t_("DIALOGS.ADDON_DETAILS.CHANGELOG_TAB"),
                "Info".to_string(),
                t_("DIALOGS.ADDON_DETAILS.IMAGES_TAB"),
            ];
            let mut lines: Vec<Line> = vec![
                Line::from(vec![Span::styled(name, Style::new().fg(t.accent).bold()), Span::raw(format!("  by {author}"))]),
                Line::from(Span::styled(format!("{provider} · {ext_id} · {url}"), Style::new().fg(t.dim))),
                Line::from(tabs.iter().enumerate().map(|(i, n)| {
                    let s = format!(" {} {n} ", i + 1);
                    if i == d.tab { Span::styled(s, Style::new().fg(t.accent).add_modifier(Modifier::REVERSED)) } else { Span::raw(s) }
                }).collect::<Vec<_>>()),
                Line::raw(""),
            ];
            for dep in &d.deps {
                lines.push(Line::styled(format!("requires: {dep}"), Style::new().fg(t.warn)));
            }
            let body: String = match d.tab {
                0 => d.description.clone(),
                1 => if d.changelog.is_empty() { "No changelog available.".into() } else { d.changelog.clone() },
                2 => detail_info(d),
                _ => {
                    let urls = d.addon.as_ref().map(|a| a.screenshot_urls.clone()).or_else(|| d.result.as_ref().map(|s| s.screenshot_urls.clone())).unwrap_or_default();
                    if urls.is_empty() { "No images.".into() } else { urls.iter().enumerate().map(|(i, u)| format!("{}. {u}", i + 1)).collect::<Vec<_>>().join("\n") + "\n\nPress o to open the first image." }
                }
            };
            let mut text = Text::from(lines);
            text.extend(Text::raw(body));
            let footer = if d.addon.is_some() { " 1-4 tabs · w website · o open image · u update · x remove · Esc " } else { " 1-4 tabs · w website · i install · Esc " };
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).scroll((d.scroll, 0)).block(block(" Details ".into()).title_bottom(footer)), r);
        }
    }
}

fn detail_info(d: &crate::app::DetailView) -> String {
    if let Some(a) = &d.addon {
        let mut s = vec![
            format!("Installed version: {}", a.installed_version.clone().unwrap_or_default()),
            format!("Latest version:    {}", a.latest_version.clone().unwrap_or_default()),
            format!("Channel:           {:?}", a.channel_type),
            format!("Ignored:           {}", a.is_ignored),
            format!("Auto update:       {} (notifications {})", a.auto_update_enabled, a.auto_update_notifications_enabled),
            format!("Folders:           {}", a.installed_folder_list.join(", ")),
            format!("Game version:      {}", a.game_version.join(", ")),
            format!("Installed at:      {}", a.installed_at.map(|d| d.to_rfc2822()).unwrap_or_default()),
            format!("Released at:       {}", a.released_at.map(|d| d.to_rfc2822()).unwrap_or_default()),
        ];
        if !a.external_ids.is_empty() {
            s.push(format!("Also on:           {}", a.external_ids.iter().map(|e| format!("{}:{}", e.provider_name, e.id)).collect::<Vec<_>>().join(", ")));
        }
        for fl in &a.funding_links {
            s.push(format!("Support ({}):  {}", fl.platform, fl.url));
        }
        if let Some(r) = a.ignore_reason {
            s.push(format!("Ignore reason:     {r:?}"));
        }
        s.join("\n")
    } else if let Some(r) = &d.result {
        let mut s: Vec<String> = r
            .files
            .iter()
            .map(|f| format!("{:?}  {}  {}  {}", f.channel_type, f.version, f.game_version, f.release_date.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()))
            .collect();
        for fl in &r.funding_links {
            s.push(format!("Support ({}): {}", fl.platform, fl.url));
        }
        s.join("\n")
    } else {
        String::new()
    }
}

const HELP: &str = "\
Global
  1 2 3 / Tab     switch My Addons, Get Addons, Options
  m               menu with every action
  + - =           zoom in / out / reset (Terminal.app)
  [  ]            previous / next WoW client (on Options: previous / next section)
  ?               this help            E  show all errors     q / Ctrl-C  quit

My Addons
  ↑↓ PgUp PgDn    move                 Space   select (multi)   Ctrl-A  select all
  /               filter               ⏎ / d   details
  u               update selected      A       update all       Ctrl-U  update all clients
  s               check for updates    r       rescan folders
  i  a  n         toggle ignore / auto update / update notifications
  c  p            change channel / switch provider
  R  x            reinstall / remove (confirm; `d` also removes dependencies)
  o  w            open folder / view on provider website
  e  I  b         export / import / WTF backups
  < >             change sort column      ,  reverse sort

Get Addons
  /  g  x         search / browse category / reset to featured
  ⏎  i  U         details / install / install from URL, wowup:// or curseforge:// link

Options
  Clients:        ⏎ select · d detect · a add · e label · c channel · u auto-update · K/J move · o open · x remove
  Providers:      ⏎ toggle provider or edit API key / token
  WTF Explorer:   r refresh · o open folder · b backups (red = SavedVariables for missing addons)
";

/// Stable / Beta / Alpha picker items, translated.
pub fn channel_items() -> Vec<(String, String)> {
    [AddonChannelType::Stable, AddonChannelType::Beta, AddonChannelType::Alpha]
        .into_iter()
        .map(|c| (channel_text(c), (c as u8).to_string()))
        .collect()
}
