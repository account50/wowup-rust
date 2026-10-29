---
name: sync-upstream
description: Pull the latest upstream WowUp (github.com/WowUp/WowUp), find what changed since our parity commit, and port those features into WowUp.rs. Use when asked to sync, update from upstream, check feature parity, or "pull in the newest WowUp features".
---

# Sync WowUp.rs with upstream WowUp

WowUp.rs is a GPL-3.0 Rust port of WowUp. `upstream/PARITY` records the upstream commit we match;
`upstream/map.tsv` says which Rust file ports each upstream file.

## Steps

1. Run `scripts/sync-upstream.sh --status`. If it reports parity, say so and stop.
2. Run `scripts/sync-upstream.sh` (set `UPSTREAM_BRANCH=develop` only if the user wants unreleased work).
   It writes `upstream/SYNC_REPORT.md` and per-file diffs in `.upstream/diffs/`.
3. Read the report's commit log first to understand intent, then work the "Files to port" table top to
   bottom. For each row read its diff and the Rust target, then port the *behaviour*:
   - Provider/API changes → `crates/core/src/providers/*` (update serde structs, URLs, matching rules).
   - Scan/sync/install/remove logic → `crates/core/src/addons.rs`; toc rules → `toc.rs`; fingerprints → `fingerprint.rs`.
   - Angular pages/dialogs/options → the TUI (`crates/tui/src/app.rs` state+actions, `ui.rs` drawing,
     `main.rs` keys). Map buttons and menu items to keys and `Modal` variants; add help text in `ui.rs` HELP.
   - Purely Electron/Angular plumbing, tests of TS-only code, CI, version bumps: no port needed.
   Tick each box in the report as you go.
4. Project rules that override upstream:
   - No ads, no ad-supplied tokens, no telemetry, no WowUp account/push.
   - Wago and CurseForge work only with keys the user enters (Preferences); never bundle a key.
   - Keep the GPL header (`// SPDX-License-Identifier: GPL-3.0-or-later`) on new files.
5. If upstream adds a file that the map routes to `review`, add a line to `upstream/map.tsv` for it.
6. Verify: `cargo test --workspace`, `cargo build --workspace`, and for install/remove-path changes run the
   sandbox check in `docs/TESTING.md`.
7. Update `docs/PARITY.md` with any new user-visible feature, then run
   `scripts/sync-upstream.sh --mark-synced` and rebuild (`scripts/install-macos.sh` reinstalls the app).
8. Summarize for the user: what was ported, what was skipped and why.
