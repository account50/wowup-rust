// SPDX-License-Identifier: GPL-3.0-or-later
//! User preferences, mirroring the keys in WowUp's `constants.ts` that still make sense without
//! Electron. Ads, analytics and the WowUp account are intentionally not ported.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    /// Provider name -> enabled (`ADDON_PROVIDER_ENABLED` keys).
    pub provider_enabled: BTreeMap<String, bool>,
    /// CurseForge API key. Without one CurseForge stays disabled (the open source build ships none).
    pub curseforge_api_key: Option<String>,
    /// Wago Addons API key. WowUp obtained this through its ad-supported flow; here it is user supplied
    /// (Wago subscribers can create one at addons.wago.io).
    pub wago_api_key: Option<String>,
    /// Optional GitHub personal access token to lift the anonymous rate limit.
    pub github_token: Option<String>,
    /// Check for and apply auto updates on this interval while the TUI is open (`autoUpdateIntervalMs`).
    pub auto_update_interval_minutes: u64,
    /// Show a notification when an addon is auto updated (`ENABLE_SYSTEM_NOTIFICATIONS_PREFERENCE_KEY`).
    pub enable_system_notifications: bool,
    /// Rescan installed addons when the app starts (`RESCAN_ON_START` style behaviour).
    pub rescan_on_start: bool,
    /// Back up WTF folders before addon updates (`ENABLE_WTF_BACKUPS`).
    pub enable_wtf_backups: bool,
    /// How many WTF backups to keep per installation.
    pub wtf_backup_count: usize,
    /// Show addons that are part of another addon (`X-Part-Of`) as their own rows.
    pub show_addon_children: bool,
    /// Columns hidden in the My Addons table.
    pub hidden_columns: Vec<String>,
    /// Sort column and direction in the My Addons table.
    pub my_addons_sort: Option<(String, bool)>,
    /// Treat `alpha`/`beta` installs per installation, defaults set on each installation instead.
    pub use_symlink_mode: bool,
    /// UI theme name.
    pub theme: String,
    /// Language code for UI strings (`selected_language`); English by default, empty follows the system locale.
    pub language: String,
    /// Open WowUp.rs at login (`start_with_system`).
    pub start_with_system: bool,
    /// At login run auto-updates in the background instead of opening the window (`start_minimized`,
    /// the counterpart of upstream starting hidden in the tray).
    pub start_minimized: bool,
    /// Force IPv4 connections (`disable_ipv6`); applies on next start.
    pub disable_ipv6: bool,
    /// Reopen the addon details on the last used tab (`keep_addon_detail_tab`).
    pub keep_detail_tab: bool,
    pub last_detail_tab: usize,
    /// Full-rate rendering (`use_hardware_acceleration`); off redraws at a low-power rate.
    pub use_hardware_acceleration: bool,
    /// Show the update count in the terminal title (`enable_app_badge`).
    pub enable_app_badge: bool,
    /// Domains whose links open without a confirmation (`trusted_domains`).
    pub trusted_domains: Vec<String>,
    /// App version whose patch notes were last shown (`update_notes_popup_version`).
    pub last_version_seen: Option<String>,
    /// Terminal window bounds [left, top, right, bottom] at last quit (`main-window-state`).
    pub window_bounds: Option<[i32; 4]>,
    /// Zoom factor (`zoom_factor`), applied to the terminal font relative to `base_font_size`.
    pub zoom_factor: f64,
    /// Terminal font size at zoom 1.0, captured the first time zoom is used.
    pub base_font_size: Option<f64>,
}

impl Default for Preferences {
    fn default() -> Self {
        let mut provider_enabled = BTreeMap::new();
        for p in ["WowUpHub", "GitHub", "WowInterface", "TukUI", "Zip", "RaiderIO", "WowUpCompanion"] {
            provider_enabled.insert(p.to_string(), true);
        }
        provider_enabled.insert("Curse".into(), false);
        provider_enabled.insert("Wago".into(), false);
        Self {
            provider_enabled,
            curseforge_api_key: None,
            wago_api_key: None,
            github_token: None,
            auto_update_interval_minutes: 60,
            enable_system_notifications: true,
            rescan_on_start: false,
            enable_wtf_backups: true,
            wtf_backup_count: 5,
            show_addon_children: false,
            hidden_columns: vec![],
            my_addons_sort: None,
            use_symlink_mode: false,
            theme: "horde".into(),
            language: "en".into(),
            start_with_system: false,
            start_minimized: false,
            disable_ipv6: false,
            keep_detail_tab: false,
            last_detail_tab: 0,
            enable_app_badge: true,
            use_hardware_acceleration: true,
            trusted_domains: ["wowup.io", "dev.wowup.io", "discord.gg", "www.patreon.com", "github.com", "wago.io", "addons.wago.io"]
                .map(String::from)
                .to_vec(),
            last_version_seen: None,
            window_bounds: None,
            zoom_factor: 1.0,
            base_font_size: None,
        }
    }
}

impl Preferences {
    /// Whether a link's host is trusted (subdomains of a trusted domain count).
    pub fn is_trusted_url(&self, url: &str) -> bool {
        let Some(host) = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)) else { return false };
        self.trusted_domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
    }

    pub fn is_provider_enabled(&self, name: &str) -> bool {
        self.provider_enabled.get(name).copied().unwrap_or(true)
    }
}
