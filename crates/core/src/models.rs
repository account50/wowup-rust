// WowUp.rs - World of Warcraft addon manager
// SPDX-License-Identifier: GPL-3.0-or-later
//! Data model, ported from wowup-lib-core `types.ts`, `addons.ts`, `toc.ts` and `models/warcraft.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const PROVIDER_UNKNOWN: &str = "Unknown";
pub const PROVIDER_CURSEFORGE: &str = "Curse";
pub const PROVIDER_CURSEFORGE_V2: &str = "CurseV2";
pub const PROVIDER_GITHUB: &str = "GitHub";
pub const PROVIDER_TUKUI: &str = "TukUI";
pub const PROVIDER_WOWINTERFACE: &str = "WowInterface";
pub const PROVIDER_HUB: &str = "WowUpHub";
pub const PROVIDER_HUB_LEGACY: &str = "Hub";
pub const PROVIDER_RAIDERIO: &str = "RaiderIO";
pub const PROVIDER_ZIP: &str = "Zip";
pub const PROVIDER_WOWUP_COMPANION: &str = "WowUpCompanion";
pub const PROVIDER_WAGO: &str = "Wago";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AddonChannelType {
    #[default]
    Stable,
    Beta,
    Alpha,
}

impl AddonChannelType {
    /// A file on `file` channel is acceptable for an addon tracking `self` (Alpha accepts everything).
    pub fn accepts(self, file: AddonChannelType) -> bool {
        (file as u8) <= (self as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddonDependencyType {
    Embedded = 1,
    Required = 2,
    Optional = 3,
    Other = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddonWarningType {
    GameVersionTocMissing,
    MissingOnProvider,
    NoProviderFiles,
    TocNameMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonIgnoreReason {
    GitRepo,
    MissingDependency,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WowClientType {
    Retail,
    Classic,
    RetailPtr,
    ClassicPtr,
    Beta,
    ClassicBeta,
    ClassicEra,
    ClassicEraPtr,
    RetailXPtr,
    Anniversary,
    Forever,
    None,
}

impl WowClientType {
    pub const ALL: [WowClientType; 11] = [
        WowClientType::Retail,
        WowClientType::Classic,
        WowClientType::RetailPtr,
        WowClientType::ClassicPtr,
        WowClientType::Beta,
        WowClientType::ClassicBeta,
        WowClientType::ClassicEra,
        WowClientType::ClassicEraPtr,
        WowClientType::RetailXPtr,
        WowClientType::Anniversary,
        WowClientType::Forever,
    ];

    pub fn game_type(self) -> WowGameType {
        use WowClientType::*;
        match self {
            ClassicEra | ClassicEraPtr => WowGameType::Classic,
            Classic | ClassicPtr => WowGameType::Mists,
            Forever | ClassicBeta => WowGameType::Forever,
            Anniversary => WowGameType::BurningCrusade,
            _ => WowGameType::Retail,
        }
    }

    pub fn display_name(self) -> &'static str {
        use WowClientType::*;
        match self {
            Retail => "Retail",
            Classic => "Classic",
            RetailPtr => "Retail PTR",
            ClassicPtr => "Classic PTR",
            Beta => "Beta",
            ClassicBeta => "Classic Beta",
            ClassicEra => "Classic Era",
            ClassicEraPtr => "Classic Era PTR",
            RetailXPtr => "Retail X-PTR",
            Anniversary => "Anniversary",
            Forever => "Forever",
            None => "None",
        }
    }

    /// The folder Battle.net installs this client into, e.g. `_retail_`.
    pub fn folder_name(self) -> &'static str {
        use WowClientType::*;
        match self {
            Retail => "_retail_",
            Classic => "_classic_",
            RetailPtr => "_ptr_",
            ClassicPtr => "_classic_ptr_",
            Beta => "_beta_",
            ClassicBeta => "_classic_beta_",
            ClassicEra => "_classic_era_",
            ClassicEraPtr => "_classic_era_ptr_",
            RetailXPtr => "_xptr_",
            Anniversary => "_anniversary_",
            Forever => "_forever_",
            None => "",
        }
    }

    pub fn from_folder_name(name: &str) -> Option<WowClientType> {
        Self::ALL.into_iter().find(|c| c.folder_name().eq_ignore_ascii_case(name))
    }

    /// Battle.net product code, as found in `product.db`.
    pub fn from_product_code(code: &str) -> Option<WowClientType> {
        use WowClientType::*;
        Some(match code {
            "wow" => Retail,
            "wow_classic" => Classic,
            "wowt" => RetailPtr,
            "wow_classic_ptr" => ClassicPtr,
            "wow_beta" => Beta,
            "wow_classic_beta" => ClassicBeta,
            "wow_classic_era" => ClassicEra,
            "wow_classic_era_ptr" => ClassicEraPtr,
            "wowxptr" => RetailXPtr,
            "wow_anniversary" => Anniversary,
            "wow_forever" => Forever,
            _ => return Option::None,
        })
    }

    /// Expansion grouping (`getWowClientGroupForType`).
    pub fn group(self) -> WowClientGroup {
        match self.game_type() {
            WowGameType::Retail => WowClientGroup::Retail,
            WowGameType::Classic => WowClientGroup::Classic,
            WowGameType::BurningCrusade => WowClientGroup::BurningCrusade,
            WowGameType::Wotlk => WowClientGroup::Wotlk,
            WowGameType::Cata => WowClientGroup::Cata,
            WowGameType::Mists => WowClientGroup::Mists,
            WowGameType::Forever => WowClientGroup::Forever,
        }
    }

    /// Interface version prefix, used to pick release files (`WowMajorVersion`).
    pub fn major_version(self) -> &'static str {
        use WowClientType::*;
        match self {
            ClassicEra | ClassicEraPtr => "1.1",
            Classic | ClassicPtr => "5.",
            Forever | ClassicBeta => "1.6",
            Anniversary => "2.",
            Beta | RetailPtr | RetailXPtr => "12.",
            _ => "11.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WowClientGroup {
    Retail,
    BurningCrusade,
    Classic,
    Wotlk,
    Cata,
    Mists,
    Forever,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WowGameType {
    Retail,
    Classic,
    BurningCrusade,
    Wotlk,
    Cata,
    Mists,
    Forever,
}

impl WowGameType {
    pub const ALL: [WowGameType; 7] = [
        WowGameType::Retail,
        WowGameType::Classic,
        WowGameType::BurningCrusade,
        WowGameType::Wotlk,
        WowGameType::Cata,
        WowGameType::Mists,
        WowGameType::Forever,
    ];

    /// The string the WowUp Hub API uses.
    pub fn as_str(self) -> &'static str {
        match self {
            WowGameType::Retail => "retail",
            WowGameType::Classic => "classic",
            WowGameType::BurningCrusade => "burningCrusade",
            WowGameType::Wotlk => "wotlk",
            WowGameType::Cata => "cata",
            WowGameType::Mists => "mists",
            WowGameType::Forever => "forever",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddonCategory {
    Unknown = 0,
    AllAddons = 1,
    Achievements,
    ActionBars,
    AuctionEconomy,
    BagsInventory,
    BossEncounters,
    BuffsDebuffs,
    Bundles,
    ChatCommunication,
    Class,
    Combat,
    Companions,
    DataExport,
    DevelopmentTools,
    Guild,
    Libraries,
    Mail,
    MapMinimap,
    Miscellaneous,
    Missions,
    Plugins,
    Professions,
    Pvp,
    QuestsLeveling,
    Roleplay,
    Tooltips,
    UnitFrames,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Toc {
    pub file_name: String,
    pub file_path: String,
    pub interface: Vec<String>,
    pub title: Option<String>,
    /// Titles per game type, only set when the toc guards titles with `[AllowLoadGameType ...]`.
    pub titles: Option<std::collections::BTreeMap<String, String>>,
    pub author: Option<String>,
    pub website: Option<String>,
    pub version: Option<String>,
    pub part_of: Option<String>,
    pub category: Option<String>,
    pub localizations: Option<String>,
    pub dependencies: Option<String>,
    pub curse_project_id: Option<String>,
    pub wow_interface_id: Option<String>,
    pub wago_addon_id: Option<String>,
    pub tuk_ui_project_id: Option<String>,
    pub tuk_ui_project_folders: Option<String>,
    pub load_on_demand: Option<String>,
    pub dependency_list: Vec<String>,
    pub addon_provider: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddonFundingLink {
    pub platform: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddonDependency {
    pub external_addon_id: String,
    #[serde(rename = "type")]
    pub dep_type: AddonDependencyType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddonExternalId {
    pub provider_name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonSearchResultFile {
    pub external_id: Option<String>,
    pub channel_type: AddonChannelType,
    pub version: String,
    pub folders: Vec<String>,
    pub game_version: String,
    pub download_url: String,
    pub release_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub dependencies: Vec<AddonDependency>,
    pub changelog: Option<String>,
    pub title: Option<String>,
    pub authors: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddonSearchResult {
    pub author: String,
    pub download_count: Option<u64>,
    pub external_id: String,
    pub external_url: String,
    pub files: Vec<AddonSearchResultFile>,
    pub name: String,
    pub provider_name: String,
    #[serde(default)]
    pub screenshot_urls: Vec<String>,
    pub summary: Option<String>,
    pub thumbnail_url: String,
    pub released_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub funding_links: Vec<AddonFundingLink>,
    pub changelog: Option<String>,
    #[serde(default)]
    pub externally_blocked: bool,
}

impl AddonSearchResult {
    /// The newest file acceptable for the channel (`getLatestFile`).
    pub fn latest_file(&self, channel: AddonChannelType) -> Option<&AddonSearchResultFile> {
        let mut files: Vec<&AddonSearchResultFile> =
            self.files.iter().filter(|f| channel.accepts(f.channel_type)).collect();
        files.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        files.into_iter().next().or_else(|| self.files.first())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Addon {
    pub id: String,
    pub name: String,
    pub download_url: Option<String>,
    pub installed_version: Option<String>,
    pub installed_external_release_id: Option<String>,
    pub latest_version: Option<String>,
    pub installed_at: Option<DateTime<Utc>>,
    pub external_id: Option<String>,
    pub external_channel: Option<String>,
    pub provider_name: Option<String>,
    pub provider_source: Option<String>,
    pub external_url: Option<String>,
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub game_version: Vec<String>,
    pub author: Option<String>,
    pub installed_folders: Option<String>,
    #[serde(default)]
    pub installed_folder_list: Vec<String>,
    pub is_ignored: bool,
    pub is_load_on_demand: bool,
    pub warning_type: Option<AddonWarningType>,
    pub auto_update_enabled: bool,
    pub auto_update_notifications_enabled: bool,
    pub client_type: Option<WowClientType>,
    pub channel_type: AddonChannelType,
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub funding_links: Vec<AddonFundingLink>,
    pub download_count: Option<u64>,
    pub summary: Option<String>,
    #[serde(default)]
    pub screenshot_urls: Vec<String>,
    pub released_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub external_ids: Vec<AddonExternalId>,
    pub external_latest_release_id: Option<String>,
    pub latest_changelog_version: Option<String>,
    pub latest_changelog: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<AddonDependency>,
    #[serde(default)]
    pub missing_dependencies: Vec<String>,
    pub ignore_reason: Option<AddonIgnoreReason>,
    pub installation_id: Option<String>,
}

impl Addon {
    pub fn provider(&self) -> &str {
        self.provider_name.as_deref().unwrap_or(PROVIDER_UNKNOWN)
    }

    /// Port of `AddonUtils.needsUpdate`: a known provider addon whose installed version or release
    /// differs from the latest one.
    pub fn needs_update(&self) -> bool {
        if self.is_ignored || self.provider() == PROVIDER_UNKNOWN {
            return false;
        }
        if let Some(latest) = self.external_latest_release_id.as_deref().filter(|l| !l.is_empty()) {
            if Some(latest) != self.installed_external_release_id.as_deref() {
                return true;
            }
        }
        let strip_v = |s: &str| s.strip_prefix(['v', 'V']).unwrap_or(s).to_string();
        let installed = strip_v(self.installed_version.as_deref().unwrap_or(""));
        let latest = strip_v(self.latest_version.as_deref().unwrap_or(""));
        !installed.is_empty() && installed != latest
    }

    /// Never installed through the app (`needsInstall`).
    pub fn needs_install(&self) -> bool {
        self.installed_version.as_deref().unwrap_or("").is_empty()
    }

    pub fn is_unknown(&self) -> bool {
        self.provider() == PROVIDER_UNKNOWN
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddonScanResult {
    pub source: String,
    pub file_count: usize,
    pub fingerprint: String,
    pub fingerprint_num: u32,
    pub folder_name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonFolder {
    pub name: String,
    pub path: String,
    pub ignore_reason: Option<AddonIgnoreReason>,
    pub tocs: Vec<Toc>,
    pub matching_addon: Option<Addon>,
    pub modified_at: Option<DateTime<Utc>>,
    pub cf_scan_results: Option<AddonScanResult>,
    pub wowup_scan_results: Option<AddonScanResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WowInstallation {
    pub id: String,
    pub client_type: WowClientType,
    /// Path to the client folder, e.g. `.../World of Warcraft/_retail_`.
    pub location: String,
    pub label: String,
    pub display_name: String,
    pub selected: bool,
    pub default_addon_channel_type: AddonChannelType,
    pub default_auto_update: bool,
    #[serde(default)]
    pub available_update_count: usize,
}

/// A WoW client found through Battle.net's product.db or a folder scan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledProduct {
    pub name: String,
    pub location: String,
    pub client_type: WowClientType,
}

impl WowInstallation {
    pub fn addons_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.location).join("Interface").join("AddOns")
    }
}

/// Result of a protocol link lookup (`wowup://`, `curseforge://`, `wago-app://`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolSearchResult {
    pub protocol: String,
    pub protocol_addon_id: Option<String>,
    pub protocol_release_id: Option<String>,
    #[serde(default)]
    pub valid_client_types: Vec<WowClientType>,
    pub search_result: AddonSearchResult,
}

/// Split a packed interface number like `110002` into `11.0.2` (`getGameVersion`).
pub fn game_version(interface: &str) -> String {
    if interface.is_empty() {
        return "0.0.0".into();
    }
    if interface.contains('.') {
        return interface.to_string();
    }
    let n = interface.len();
    if n < 5 {
        return interface.to_string();
    }
    let p = |s: &str| s.parse::<u32>().unwrap_or(0).to_string();
    format!("{}.{}.{}", p(&interface[..n - 4]), p(&interface[n - 4..n - 2]), p(&interface[n - 2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_versions() {
        assert_eq!(game_version("110002"), "11.0.2");
        assert_eq!(game_version("11507"), "1.15.7");
        assert_eq!(game_version(""), "0.0.0");
        assert_eq!(game_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn channel_accepts() {
        assert!(AddonChannelType::Beta.accepts(AddonChannelType::Stable));
        assert!(!AddonChannelType::Stable.accepts(AddonChannelType::Beta));
    }
}
