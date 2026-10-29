# WowUp.rs

A World of Warcraft addon manager for the terminal, written in Rust. It is a port of
[WowUp](https://github.com/WowUp/WowUp) with the same providers and addon pipeline, licensed under the
GNU GPL v3 like the original, and with no ads or telemetry.

- **TUI** (`wowup-tui`): My Addons, Get Addons and Options tabs, like WowUp's window.
- **CLI** (`wowup`): every action is scriptable (`wowup update --all`, `wowup install <url>`, …).
- **Providers**: WowUp Hub, GitHub, WowInterface, TukUI, direct .zip, plus CurseForge and Wago with your
  own API keys.

See [docs/PARITY.md](docs/PARITY.md) for the feature-by-feature comparison with WowUp.

## Install on macOS

Requires a Rust toolchain (`rustup`).

```bash
scripts/install-macos.sh
```

This builds release binaries and installs `/Applications/WowUp.rs.app`. Opening the app starts the TUI in
Terminal; the app is also registered for `wowup://` and `curseforge://` links. The `wowup` and
`wowup-tui` commands are linked into `~/.local/bin`.

On first start it finds your WoW installs through Battle.net. Press `?` in the TUI for keys.
Options → Application has language (WowUp's 13 translations), launch at login, and "launch minimized",
which runs auto-updates in the background at login and on the update interval.

## Settings worth knowing

| Setting | How |
|---|---|
| GitHub token (avoids rate limits) | Options → Addon Providers, or `wowup config set github_token …` |
| CurseForge API key | Options → Addon Providers, or `wowup config set curseforge_api_key …` |
| Wago API key | Options → Addon Providers, or `wowup config set wago_api_key …` |
| Data folder | `~/Library/Application Support/WowUp.rs` (override with `WOWUP_DATA_DIR`) |

## Staying in sync with upstream WowUp

`upstream/PARITY` pins the upstream commit this port matches. To bring in new WowUp features:

```bash
scripts/sync-upstream.sh --status   # are we behind?
scripts/sync-upstream.sh            # writes upstream/SYNC_REPORT.md: commits + files to port, routed via upstream/map.tsv
# …port the changes, run cargo test --workspace…
scripts/sync-upstream.sh --mark-synced
```

In Claude Code, the `sync-upstream` skill runs this whole loop: ask it to "sync with upstream WowUp".

## Layout

| Path | What |
|---|---|
| `crates/core` | Library: models, toc, fingerprints, providers, scan/sync/install/remove, storage, WTF, import/export |
| `crates/cli` | `wowup` command |
| `crates/tui` | `wowup-tui` terminal UI (ratatui) |
| `packaging/macos` | App bundle launcher (AppleScript applet for URL events) and icon |
| `upstream/` | Parity pin, upstream→Rust path map, generated sync report |

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE). WowUp is © its authors and also GPL-3.0;
the icon and in-game data addon icon come from the WowUp project.
