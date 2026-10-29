#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Keep WowUp.rs in feature parity with upstream WowUp.
#
# Fetches github.com/WowUp/WowUp, diffs everything since the commit recorded in upstream/PARITY,
# routes each changed upstream file to the Rust code that ports it (upstream/map.tsv), and writes
# a porting worklist to upstream/SYNC_REPORT.md with per-file diffs under .upstream/diffs/.
#
#   scripts/sync-upstream.sh                 fetch + write the report
#   scripts/sync-upstream.sh --status        one-line summary (exit 1 when behind)
#   scripts/sync-upstream.sh --mark-synced   after porting: record the upstream head as the new parity point
#
# Env: UPSTREAM_URL (default WowUp/WowUp), UPSTREAM_BRANCH (default master; `develop` for unreleased work).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
URL="${UPSTREAM_URL:-https://github.com/WowUp/WowUp.git}"
BRANCH="${UPSTREAM_BRANCH:-master}"
CACHE="$ROOT/.upstream"
REPO="$CACHE/repo"
PARITY_FILE="$ROOT/upstream/PARITY"
MAP="$ROOT/upstream/map.tsv"
REPORT="$ROOT/upstream/SYNC_REPORT.md"
MODE="${1:-report}"

mkdir -p "$CACHE"
if [[ ! -d "$REPO/.git" ]]; then
  echo "==> Cloning $URL" >&2
  git clone --quiet --filter=blob:none --no-checkout "$URL" "$REPO"
fi
git -C "$REPO" fetch --quiet origin "$BRANCH"
TO="$(git -C "$REPO" rev-parse "origin/$BRANCH")"
FROM="$(tr -d '[:space:]' < "$PARITY_FILE")"

if ! git -C "$REPO" cat-file -e "$FROM^{commit}" 2>/dev/null; then
  echo "parity commit $FROM not found upstream (history rewritten?)" >&2
  exit 2
fi

behind="$(git -C "$REPO" rev-list --count "$FROM..$TO")"

case "$MODE" in
  --status)
    if [[ "$behind" == 0 ]]; then
      echo "in parity with upstream $BRANCH @ ${TO:0:10}"
      exit 0
    fi
    echo "$behind upstream commit(s) to review (${FROM:0:10}..${TO:0:10})"
    exit 1
    ;;
  --mark-synced)
    target="${2:-$TO}"
    echo "$target" > "$PARITY_FILE"
    echo "parity commit set to $target"
    echo "rebuild to show it in About: cargo build --release (or scripts/install-macos.sh)"
    exit 0
    ;;
  report) ;;
  *) echo "unknown option $MODE" >&2; exit 1 ;;
esac

if [[ "$behind" == 0 ]]; then
  echo "Already in parity with upstream $BRANCH @ ${TO:0:10}."
  exit 0
fi

# Load the path map (first matching glob wins).
globs=(); targets=()
while IFS=$'\t' read -r glob target; do
  [[ -z "$glob" || "$glob" == \#* ]] && continue
  globs+=("$glob"); targets+=("$target")
done < "$MAP"

route() {
  local p="$1" i
  for i in "${!globs[@]}"; do
    # shellcheck disable=SC2053
    if [[ "$p" == ${globs[$i]} ]]; then
      echo "${targets[$i]}"
      return
    fi
  done
  echo "review"
}

rm -rf "$CACHE/diffs"
mkdir -p "$CACHE/diffs"
: > "$CACHE/port.diff"

ver() { git -C "$REPO" show "$1:wowup-electron/package.json" 2>/dev/null | grep -m1 '"version"' | cut -d'"' -f4; }
from_ver="$(ver "$FROM")"; to_ver="$(ver "$TO")"

port_rows=""; skip_rows=""; n_port=0; n_skip=0
while IFS=$'\t' read -r status path rest; do
  # Renames report "R100 old new"; route on the new path.
  [[ "$status" == R* ]] && path="$rest"
  case "$path" in *.png|*.jpg|*.svg|*.ico|*.icns|*.gif|*.woff|*.woff2|*.ttf|*package-lock.json) continue ;; esac
  target="$(route "$path")"
  stat="$(git -C "$REPO" diff --numstat "$FROM" "$TO" -- "$path" | awk '{print "+"$1" -"$2}')"
  if [[ "$target" == skip:* ]]; then
    skip_rows+="| \`$path\` | $status | ${target#skip: } |"$'\n'
    n_skip=$((n_skip + 1))
  else
    safe="${path//\//__}"
    git -C "$REPO" diff "$FROM" "$TO" -- "$path" > "$CACHE/diffs/$safe.diff"
    cat "$CACHE/diffs/$safe.diff" >> "$CACHE/port.diff"
    port_rows+="| [ ] | \`$path\` | $status $stat | $target | \`.upstream/diffs/$safe.diff\` |"$'\n'
    n_port=$((n_port + 1))
  fi
done < <(git -C "$REPO" diff --name-status "$FROM" "$TO")

{
  echo "# Upstream sync report"
  echo
  echo "Generated $(date '+%Y-%m-%d %H:%M') by \`scripts/sync-upstream.sh\`."
  echo
  echo "- Upstream: $URL ($BRANCH)"
  echo "- Parity commit: \`$FROM\` (WowUp ${from_ver:-?})"
  echo "- Upstream head: \`$TO\` (WowUp ${to_ver:-?})"
  echo "- Commits behind: **$behind** · files to port: **$n_port** · skipped by design: $n_skip"
  echo
  echo "## How to work this list"
  echo
  echo "1. Read the commit log below for intent, then each diff. The combined diff is \`.upstream/port.diff\`."
  echo "2. Port behaviour into the Rust target. Keep this project's rules: GPL-3.0, no ads, no telemetry,"
  echo "   Wago/CurseForge only with user-supplied keys, terminal UI instead of Angular screens."
  echo "3. Add or update tests next to the change; run \`cargo test --workspace\`."
  echo "4. Tick the box. When all are done: \`scripts/sync-upstream.sh --mark-synced\`."
  echo "5. Note user-visible changes in \`docs/PARITY.md\`."
  echo
  echo "## Upstream commits"
  echo
  echo '```'
  git -C "$REPO" log --no-merges --format='%h %ad %s' --date=short "$FROM..$TO"
  echo '```'
  echo
  echo "## Files to port"
  echo
  echo "| Done | Upstream file | Change | Rust target | Diff |"
  echo "|---|---|---|---|---|"
  printf '%s' "$port_rows"
  echo
  echo "## Skipped by design"
  echo
  echo "| Upstream file | Change | Reason |"
  echo "|---|---|---|"
  printf '%s' "$skip_rows"
} > "$REPORT"

echo "$behind commit(s) behind: $n_port file(s) to port, $n_skip skipped."
echo "Report: upstream/SYNC_REPORT.md   Combined diff: .upstream/port.diff"
