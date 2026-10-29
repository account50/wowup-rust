# Testing

```bash
cargo test --workspace
```

## Sandbox end-to-end check

Exercise the real providers without touching your WoW install or app data. `WOWUP_DATA_DIR` moves
the app's data folder; the sandbox client folder stands in for `_retail_`.

```bash
S=$(mktemp -d); mkdir -p "$S/wow/_retail_/Interface/AddOns" "$S/wow/_retail_/WTF" "$S/wow/_classic_era_/Interface/AddOns"
export WOWUP_DATA_DIR="$S/data" W=./target/debug/wowup
cargo build -p wowup-cli
$W clients add "$S/wow/_retail_" && $W clients add "$S/wow/_classic_era_"
$W install WowUpHub:16                                                     # Hub (Masque)
$W install TukUI:-2                                                        # TukUI (ElvUI)
$W install https://www.wowinterface.com/downloads/info5108-Clique.html    # WowInterface
$W install https://github.com/WeakAuras/WeakAuras2                         # GitHub
$W -c "classic era" install https://github.com/WeakAuras/WeakAuras2/releases/download/5.22.1/WeakAuras-5.22.1.zip  # Zip
$W scan && $W sync && $W list            # all four re-identified, nothing falsely outdated
$W ignore Clique && $W ignore Clique --off && $W auto-update Clique && $W channel Masque beta
EXP=$($W export); $W remove Clique; $W remove ElvUI; $W import "$EXP" --apply   # remove + re-add
$W wtf backup && $W wtf list
$W info ElvUI
```

To test an update, edit `installedVersion`/`installedExternalReleaseId` of an addon in
`$S/data/addons/<installation-id>.json`, then `$W list --updates` and `$W update <name>`.

The TUI can be driven the same way: `WOWUP_DATA_DIR=$S/data ./target/debug/wowup-tui`.
