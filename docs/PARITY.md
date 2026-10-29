# Feature parity with WowUp

Upstream: WowUp 2.24.1, commit recorded in [`upstream/PARITY`](../upstream/PARITY).
Keep this file current when porting upstream changes (`scripts/sync-upstream.sh`).

Legend: ✅ ported · ➖ removed on purpose · ⏳ not ported yet

## Addon providers

| Provider | Status | Notes |
|---|---|---|
| WowUp Hub | ✅ | Fingerprint scan, batch sync, search, featured, categories, `wowup://` links, changelog/description |
| GitHub | ✅ | release.json flavors, asset-name rules per client, prerelease handling, PAT, rate-limit errors |
| WowInterface | ✅ | v4 API with v3 fallback, toc `X-WoWI-ID` matching, URL install |
| TukUI | ✅ | toc project id + title-similarity matching, sub-folder grouping |
| CurseForge | ✅ | Fingerprint matching, featured, categories, search, URL, `curseforge://` links — **needs your own API key**. Verified against a mock CF API (`tests/curseforge_mock.rs`): scan-match, sync, update, install with dependency, remove |
| Wago | ✅ | Fingerprint `_match`, recents sync, search, popular — **needs your own Wago API key**; the ad-supplied token path is removed. Verified against a mock Wago API (`tests/wago_mock.rs`) |
| Zip (direct URL) | ✅ | Content-type check, force-ignored like upstream |
| RaiderIO / WowUp Companion | ✅ | Local grouping of multi-folder addons, kept ignored |

## Addon management

| Feature | Status | Where |
|---|---|---|
| Detect installs (Battle.net product.db, default folders, Lutris) | ✅ | `installations.rs` |
| Add/remove/select/reorder/label clients, default channel & auto-update per client | ✅ | Options → World of Warcraft |
| TOC parsing incl. per-flavor tocs and `[AllowLoadGameType]` titles | ✅ | `toc.rs` |
| WowUp (md5) and CurseForge (murmur2) folder fingerprints | ✅ | `fingerprint.rs` |
| Scan / rescan with provider priority, git-repo ignore, LoD child folders, toc-name warning | ✅ | `addons.rs` |
| Sync (check for updates), warnings for missing addon / no files | ✅ | `s` |
| Install: download (provider auth, retries with URL refresh), backup, unzip (zip-slip safe), rollback | ✅ | |
| Required-dependency auto-install, remove with dependencies | ✅ | `x` then `d` |
| Update one / selected / all / all clients | ✅ | `u`, `A`, `Ctrl-U` |
| Reinstall, ignore, auto update, update notifications, release channel, switch provider | ✅ | `R i a n c p` |
| Learn hub id of addons from other providers | ✅ | |
| Install from URL, `wowup://` and `curseforge://` links (macOS URL handler) | ✅ | `U`, app bundle |
| Import / export (WowUp-compatible share strings) | ✅ | `e`, `I` |
| WTF backups: create, list, restore (validated), delete, prune | ✅ | `b` |
| WTF explorer (orphaned SavedVariables in red) | ✅ | Options → WTF Explorer |
| In-game update list (`wowup_data_addon` for the "Addon Update Notifications" addon) | ✅ | `companion.rs` |
| Hourly auto-update with system notifications | ✅ | while the TUI is open; `wowup auto-update-pass` for cron/launchd |
| Symlinked addon folders | ✅ | Options → Application |
| Addon details: description, changelog, info, images, funding links | ✅ | `⏎` |
| Search, featured, categories | ✅ | Get Addons |
| Column hide (Released/Game/Author), sort by any column, filter | ✅ | `< > ,` and `/` |
| Show pending (dependency) installs in the grid (upstream 2.24.1) | ✅ | extra rows with live install state |
| Persist sort order (`my_addons_sort_order`) | ✅ | |
| Keep last opened details tab | ✅ | Options → Application |
| Link confirmation with trusted domains | ✅ | `t` trusts a domain |

## App shell

| Feature | Status | Notes |
|---|---|---|
| Ads (Wago ad view, Overwolf ads, consent/CMP windows) | ➖ | GPL build without ads |
| Telemetry (Application Insights) | ➖ | was already disabled upstream |
| WowUp account, Patreon login, push notifications | ➖ | disabled upstream (`FEATURE_ACCOUNTS_ENABLED = false`) |
| News panel (Warcraft Tavern) | ✅ | menu → News, `wowup news`; upstream's feed URL currently returns no items, so it shows "No news" |
| Update-count badge | ✅ | shown in the terminal window title |
| Force IPv4 (disable IPv6) | ✅ | Options → Application, applies on restart |
| Tray icon / close-to-tray | ✅ (equivalent) | "Launch minimized" runs background auto-updates at login and on the interval, with notifications |
| App menu | ✅ (equivalent) | `m` opens a menu of every action |
| Options → Debug: log files, config files, dump debug data, process metrics | ✅ | keys redacted in the dump |
| Window size and position remembered between sessions | ✅ | Terminal.app window bounds saved on quit, restored on launch |
| Zoom (0.5×–3.0× steps, persisted) | ✅ | `+` `-` `=` scale the Terminal.app font; other terminals use their own zoom keys |
| DevTools | ✅ (equivalent) | Options → Debug → Log viewer, debug data dump |
| Hardware acceleration toggle (`use_hardware_acceleration`) | ✅ (equivalent) | on = full-rate redraws; off = low-power redraw rate, the TUI's rendering cost knob |
| App self-updater | ✅ (equivalent) | menu → Update WowUp.rs, or `wowup self-update`: pulls (when tracking a remote) and reinstalls from source |
| Launch with system / start minimized | ✅ | macOS LaunchAgent (`~/Library/LaunchAgents/io.wowup.rs.plist`) |
| Translations (13 languages) | ✅ | upstream locale files and keys; help and key hints are English |
| Themes | ✅ | horde / alliance / default accent colours |
| Patch notes popup | ✅ | shown once after each version change (`CHANGELOG.md`) |
