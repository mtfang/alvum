# PMF Path — Runbook

Operational companion to `docs/superpowers/plans/2026-04-18-pmf-path.md`. The strategy doc says *what and why*; this runbook gives the *exact scripts and commands*.

**Design principle:** scripts are named by **capability**, not by rollout stage. Each script stays stable across the PMF stages (Stage 0 / 1 / 2 in the strategy doc). What changes between stages is **which config flags are set** and **which launchd jobs are loaded** — never the scripts themselves.

A user who installs for the first time today gets the same scripts as the builder who installed two months ago. Everything composes. Nothing is renamed between phases. Adding a new capability adds a new script; it never modifies the old ones.

## Repo Layout Added by This Runbook

```
alvum/
├── scripts/
│   ├── lib.sh                    ← shared path + env conventions (sourced, not executed)
│   ├── install.sh                ← one-time setup: binary, config, briefing schedule
│   ├── uninstall.sh              ← clean reverse (idempotent, --purge for full wipe)
│   ├── briefing.sh               ← generate today's briefing (also the launchd entry point)
│   ├── capture.sh                ← start | stop | status | toggle <source>
│   ├── email.sh                  ← email the latest briefing to ~/.alvum/runtime/email.txt
│   ├── status.sh                 ← one-screen health report
│   ├── view.sh                   ← open today's briefing in the default viewer
│   ├── menu-bar.sh               ← SwiftBar plugin (STOP-GAP; remove when full app ships)
│   ├── menu-bar-install.sh       ← installs SwiftBar + symlinks the plugin
│   └── menu-bar-uninstall.sh     ← removes the symlink (leaves SwiftBar itself)
└── launchd/
    ├── com.alvum.briefing.plist  ← schedules briefing.sh daily at 07:00
    └── com.alvum.capture.plist   ← runs `alvum capture` continuously (opt-in)
```

## User-Side Directory Layout (per Mac, outside the repo)

Per `docs/superpowers/specs/2026-04-18-storage-layout.md` (authoritative). Single root, three lifecycle buckets. Established by `install.sh`:

```
~/.alvum/
├── capture/                          ← GROUND TRUTH; kept indefinitely; back up
│   └── YYYY-MM-DD/
├── generated/                        ← CURRENT DERIVATION; back up
│   └── briefings/YYYY-MM-DD/
│       ├── briefing.md
│       ├── decisions.jsonl
│       └── threads.json
└── runtime/                          ← OPERATIONAL; never back up
    ├── bin/alvum                     ← release binary
    ├── config.toml                   ← the only config file
    ├── email.txt                     ← optional; if present, briefing.sh triggers email.sh
    └── logs/
        ├── briefing.out / briefing.err
        └── capture.out  / capture.err
```

The full tree (with all `generated/` subdirectories populated over time) lives in the storage-layout spec; this runbook only references the paths it touches.

## Prerequisites (one-time per Mac)

1. **macOS 14+**.
2. **Rust toolchain** (install via the Rust interactive installer from `rustup.rs` — do not pipe curl into sh in automation; have the user run the installer themselves).
3. **Claude Code CLI** installed and authenticated (`claude login`). The default provider (`cli`) shells out to `claude -p`.
4. **Mail.app default account configured** (only needed when using email delivery — the `email.sh` script uses system `mail(1)`).

---

## Pre-Runbook: One CLI Gap Fix

The current `ClaudeCodeConnector` (`crates/alvum-connector-claude/src/connector.rs:14-46`) reads all historical sessions under `~/.claude/projects`, filtered only by an upper bound (`before_ts`). To scope a daily briefing to "the last 24 hours," the connector needs a lower bound too.

**Files to modify:** `crates/alvum-connector-claude/src/{connector.rs,parser.rs}`.

**Change:** add an `after_ts: Option<DateTime<Utc>>` field mirroring the existing `before_ts`. Read from a `since` key in the connector's TOML settings. Thread through to `parser::parse_session_filtered`, which becomes `parse_session_filtered(path, after, before)`. Skip any record whose timestamp is earlier than `after`.

**Test** (in `parser.rs` tests):

```rust
#[test]
fn after_filter_excludes_earlier_records() {
    // Fixture with records at 10:00, 11:00, 12:00.
    // Parse with after = 11:30; expect only the 12:00 record.
}
```

**Commit:** `feat(connector-claude): add since timestamp filter`.

Required before `briefing.sh` can run with a 24-hour window. Two lines of trait plumbing + one parser condition + one test. ~15 min of work.

---

## Shared Module: `scripts/lib.sh`

Every script below sources this. Nothing executes from here.

```bash
#!/usr/bin/env bash
# Shared path + env conventions. Source, don't execute.
# Layout authority: docs/superpowers/specs/2026-04-18-storage-layout.md

set -euo pipefail

# Single root — override for testing; production is always $HOME/.alvum.
export ALVUM_ROOT="${ALVUM_ROOT:-$HOME/.alvum}"

# Three lifecycle buckets.
export ALVUM_CAPTURE="$ALVUM_ROOT/capture"
export ALVUM_GENERATED="$ALVUM_ROOT/generated"
export ALVUM_RUNTIME="$ALVUM_ROOT/runtime"

# Binaries + config + small state — all under runtime/.
export ALVUM_BIN="${ALVUM_BIN:-$ALVUM_RUNTIME/bin/alvum}"
export ALVUM_CONFIG_FILE="$ALVUM_RUNTIME/config.toml"
export ALVUM_EMAIL_FILE="$ALVUM_RUNTIME/email.txt"
export ALVUM_LOGS_DIR="$ALVUM_RUNTIME/logs"

# Generated data — where briefings land.
export ALVUM_BRIEFINGS_DIR="$ALVUM_GENERATED/briefings"

export ALVUM_LAUNCHAGENTS="$HOME/Library/LaunchAgents"
export ALVUM_BRIEFING_LABEL="com.alvum.briefing"
export ALVUM_CAPTURE_LABEL="com.alvum.capture"

export ALVUM_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

today()      { date +%Y-%m-%d; }
yesterday()  { date -v-1d +%Y-%m-%d; }
now_utc()    { date -u +%Y-%m-%dT%H:%M:%SZ; }

ensure_dirs() {
  mkdir -p "$ALVUM_RUNTIME/bin" "$ALVUM_RUNTIME/logs" \
           "$ALVUM_CAPTURE" "$ALVUM_BRIEFINGS_DIR" \
           "$ALVUM_LAUNCHAGENTS"
}

# Install a templated launchd plist. Arguments: <src plist> <dst plist>.
# Any @@VAR@@ in the plist is replaced with env("VAR") if set.
install_plist() {
  local src="$1" dst="$2"
  sed -e "s|@@ALVUM_ROOT@@|$ALVUM_ROOT|g" \
      -e "s|@@ALVUM_RUNTIME@@|$ALVUM_RUNTIME|g" \
      -e "s|@@ALVUM_BIN@@|$ALVUM_BIN|g" \
      -e "s|@@ALVUM_REPO@@|$ALVUM_REPO|g" \
      "$src" > "$dst"
  launchctl bootout "gui/$UID" "$dst" 2>/dev/null || true
  launchctl bootstrap "gui/$UID" "$dst"
}

unload_plist() {
  local dst="$1"
  launchctl bootout "gui/$UID" "$dst" 2>/dev/null || true
  rm -f "$dst"
}

plist_loaded() {
  local label="$1"
  launchctl list | awk -v l="$label" '$3 == l { found=1 } END { exit !found }'
}

# ---- derived-state helpers (used by status.sh, doctor.sh, and menu-bar.sh) ----

# 0 (true) if today's briefing.md exists.
briefing_fresh_today() {
  [[ -f "$ALVUM_BRIEFINGS_DIR/$(today)/briefing.md" ]]
}

# 0 (true) if capture daemon is loaded but hasn't written anything in > 2h.
# 1 (false) if capture daemon is off (nothing to be stale).
any_capture_stale() {
  plist_loaded "$ALVUM_CAPTURE_LABEL" || return 1
  local today_dir="$ALVUM_CAPTURE/$(today)"
  [[ ! -d "$today_dir" ]] && return 0
  local newest
  newest=$(find "$today_dir" -type f -print0 2>/dev/null \
    | xargs -0 stat -f "%m" 2>/dev/null \
    | sort -rn | head -1)
  [[ -z "$newest" ]] && return 0
  local age=$(( $(date +%s) - newest ))
  (( age > 7200 ))
}

# Returns "true"/"false" for a given source.
# Sources: audio-mic, audio-system, screen, claude-code.
is_source_enabled() {
  local src="$1"
  local section
  case "$src" in
    claude-code) section="connectors.claude-code" ;;
    *)           section="capture.$src" ;;
  esac
  local out
  out=$("$ALVUM_BIN" config-show 2>/dev/null \
    | awk -v sect="[$section]" '
        $0 == sect { in_s=1; next }
        in_s && /^\[/ { in_s=0 }
        in_s && /^enabled[[:space:]]*=/ { gsub(/[[:space:]]/, ""); split($0, a, "="); print a[2]; exit }
      ')
  echo "${out:-false}"
}

# Human-readable age of the newest briefing, or "never" if none exists.
last_briefing_relative() {
  local f
  for d in "$(today)" "$(yesterday)"; do
    f="$ALVUM_BRIEFINGS_DIR/$d/briefing.md"
    [[ -f "$f" ]] && break
    f=""
  done
  [[ -z "$f" ]] && { echo "never"; return; }
  local age=$(( $(date +%s) - $(stat -f "%m" "$f") ))
  if   (( age <  3600 )); then echo "$(( age / 60 ))m ago"
  elif (( age < 86400 )); then echo "$(( age / 3600 ))h ago"
  else                         echo "$(( age / 86400 ))d ago"
  fi
}

# "running" | "stopped"
capture_state() {
  if plist_loaded "$ALVUM_CAPTURE_LABEL"; then echo "running"
  else                                          echo "stopped"
  fi
}
```

---

## `scripts/install.sh`

One-time setup. Idempotent — safe to re-run. Does the minimum:
- Builds and installs the `alvum` binary
- Writes a minimal default config (claude-code enabled, screen off, no email)
- Installs and schedules the daily briefing (07:00)

Does **not** start capture (that's `capture.sh start`). Does **not** configure email (that's `echo <addr> > ~/.alvum/runtime/email.txt`). Enabling more capabilities is an *opt-in* action after install.

```bash
#!/usr/bin/env bash
# One-time setup: build binary, write config, schedule daily briefing.
# Re-run: safe. Overwrites binary, config, and plist.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> install"

# 1. Dependencies.
command -v cargo  >/dev/null || { echo "cargo not found; install Rust first"; exit 1; }
command -v claude >/dev/null || { echo "claude CLI not found; install from claude.com/download"; exit 1; }

ensure_dirs

# 2. Build and install the release binary.
echo "--> building alvum"
(cd "$ALVUM_REPO" && cargo build --release -p alvum-cli)
install -m 755 "$ALVUM_REPO/target/release/alvum-cli" "$ALVUM_BIN"
echo "    $ALVUM_BIN"

# 3. Write a minimal default config.
echo "--> writing config"
cat > "$ALVUM_CONFIG_FILE" <<EOF
# Minimal default: Claude Code only. Enable more capabilities with capture.sh / email.
[pipeline]
provider = "cli"
model = "claude-sonnet-4-6"
output_dir = "$ALVUM_BRIEFINGS_DIR"

[connectors.claude-code]
enabled = true
session_dir = "$HOME/.claude/projects"
# 'since' is overridden per-run by briefing.sh to scope to the last 24h.

[connectors.screen]
enabled = false
vision = "ocr"

[capture.audio-mic]
enabled = false
[capture.audio-system]
enabled = false
[capture.screen]
enabled = false
idle_interval_secs = 30
EOF
echo "    $ALVUM_CONFIG_FILE"

# 4. Schedule the daily briefing.
echo "--> scheduling daily briefing (07:00 local)"
install_plist \
  "$ALVUM_REPO/launchd/$ALVUM_BRIEFING_LABEL.plist" \
  "$ALVUM_LAUNCHAGENTS/$ALVUM_BRIEFING_LABEL.plist"

# 5. Dry-run config to validate.
"$ALVUM_BIN" config-show >/dev/null

# 6. Optionally install the menu-bar plugin (STOP-GAP until full app ships).
if [[ "${ALVUM_SKIP_MENU_BAR:-}" != "1" ]]; then
  read -r -p "Install the menu-bar plugin (adds a status dot + quick actions)? [Y/n] " ans
  if [[ "$ans" != "n" && "$ans" != "N" ]]; then
    "$ALVUM_REPO/scripts/menu-bar-install.sh"
  fi
fi

echo
echo "installed."
echo
echo "next:"
echo "  $ALVUM_REPO/scripts/briefing.sh        # run a briefing right now"
echo "  $ALVUM_REPO/scripts/view.sh            # open today's briefing"
echo "  $ALVUM_REPO/scripts/capture.sh start   # enable capture daemon (opt-in)"
echo "  echo you@example.com > $ALVUM_EMAIL_FILE   # enable email delivery (opt-in)"
echo "  $ALVUM_REPO/scripts/status.sh          # health check"
```

## `scripts/briefing.sh`

The capability: produce today's briefing. Also the launchd entry point.

- Idempotent: re-running overwrites today's briefing.
- Reads from **whatever connectors are enabled in config** — works with Claude Code only, or with screen data, or both. No stage-specific logic.
- If `~/.alvum/runtime/email.txt` exists, triggers `email.sh` at the end. Otherwise stays silent about email.

```bash
#!/usr/bin/env bash
# Generate today's briefing using whatever connectors are enabled in config.
# If ~/.alvum/runtime/email.txt exists, email it afterward.

set -euo pipefail
source "$(dirname "$0")/lib.sh"
ensure_dirs

TODAY=$(today)
YESTERDAY=$(yesterday)
OUT_DIR="$ALVUM_BRIEFINGS_DIR/$TODAY"
mkdir -p "$OUT_DIR"

# Scope the Claude Code connector to the last 24h.
SINCE_ISO=$(date -j -f "%Y-%m-%d %H:%M:%S" "$YESTERDAY 00:00:00" -u +"%Y-%m-%dT%H:%M:%SZ")
"$ALVUM_BIN" config-set "connectors.claude-code.since" "$SINCE_ISO" >/dev/null

# Capture dir: yesterday's data if it exists (screen daemon writes there);
# otherwise an empty directory (alvum-cli requires the flag even for claude-only).
CAPTURE_DIR="$ALVUM_CAPTURE/$YESTERDAY"
mkdir -p "$CAPTURE_DIR"

echo "[$(now_utc)] briefing start (since=$SINCE_ISO)"

"$ALVUM_BIN" extract \
  --capture-dir "$CAPTURE_DIR" \
  --output "$OUT_DIR" \
  --provider cli \
  --model claude-sonnet-4-6

echo "[$(now_utc)] briefing done -> $OUT_DIR/briefing.md"

# Email, if configured. Isolated to the email script — do not inline here.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  "$(dirname "$0")/email.sh"
fi
```

## `scripts/capture.sh`

Single script, three subcommands. Controls the capture daemon *and* the config flag that governs whether screen data is ingested.

```bash
#!/usr/bin/env bash
# Control the capture daemon and the per-source config flags.
#
# Subcommands:
#   start                        — load + bootstrap the daemon plist
#   stop                         — unload the daemon plist
#   status                       — print daemon + per-source state
#   toggle <source>              — flip a source on/off; kickstart daemon to reload
#                                  (source ∈ audio-mic | audio-system | screen | claude-code)

set -euo pipefail
source "$(dirname "$0")/lib.sh"

cmd="${1:-status}"
PLIST_SRC="$ALVUM_REPO/launchd/$ALVUM_CAPTURE_LABEL.plist"
PLIST_DST="$ALVUM_LAUNCHAGENTS/$ALVUM_CAPTURE_LABEL.plist"

# Map a source name to its config key.
source_config_key() {
  case "$1" in
    claude-code) echo "connectors.claude-code.enabled" ;;
    audio-mic|audio-system|screen)
                 echo "capture.$1.enabled" ;;
    *)           echo ""; return 1 ;;
  esac
}

case "$cmd" in
  start)
    ensure_dirs
    echo "--> starting capture daemon (reads config for enabled sources)"
    install_plist "$PLIST_SRC" "$PLIST_DST"

    cat <<EOF

capture daemon started. writing to: $ALVUM_CAPTURE/<today>/

per-source enable/disable is in $ALVUM_CONFIG_FILE; toggle with:
  capture.sh toggle <source>
where <source> ∈ { audio-mic, audio-system, screen, claude-code }.

macOS will prompt for permissions the first time each source captures.
If capture.err shows permission errors, grant in System Settings → Privacy & Security.
EOF
    ;;

  stop)
    echo "--> stopping capture daemon"
    unload_plist "$PLIST_DST"
    echo "    capture stopped (config flags left alone; restart with capture.sh start)"
    ;;

  toggle)
    src="${2:-}"
    key=$(source_config_key "$src") || {
      echo "usage: $0 toggle { audio-mic | audio-system | screen | claude-code }" >&2
      exit 2
    }
    current=$(is_source_enabled "$src")
    if [[ "$current" == "true" ]]; then
      "$ALVUM_BIN" config-set "$key" "false" >/dev/null
      echo "$src: disabled"
    else
      "$ALVUM_BIN" config-set "$key" "true" >/dev/null
      echo "$src: enabled"
    fi
    # Kick the daemon so it picks up the new config.
    if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
      launchctl kickstart -k "gui/$UID/$ALVUM_CAPTURE_LABEL" 2>/dev/null || true
    fi
    ;;

  status)
    if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
      echo "daemon:   loaded"
    else
      echo "daemon:   not loaded"
    fi
    for s in claude-code audio-mic audio-system screen; do
      on=$(is_source_enabled "$s")
      marker=$([[ "$on" == "true" ]] && echo "✓" || echo "·")
      printf "  %s %s\n" "$marker" "$s"
    done
    today_dir="$ALVUM_CAPTURE/$(today)"
    if [[ -d "$today_dir" ]]; then
      echo "today:    $(find "$today_dir" -type f | wc -l | tr -d ' ') files under $today_dir"
    else
      echo "today:    no captures"
    fi
    ;;

  *)
    echo "usage: $0 {start|stop|status|toggle <source>}" >&2
    exit 2
    ;;
esac
```

## `scripts/email.sh`

Sends the latest briefing to the address in `~/.alvum/runtime/email.txt`. Atomic: one job, usable on its own or chained from `briefing.sh`.

```bash
#!/usr/bin/env bash
# Email today's briefing to the address in ~/.alvum/runtime/email.txt.
# Idempotent — re-running re-sends the same day's briefing.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

if [[ ! -f "$ALVUM_EMAIL_FILE" ]]; then
  echo "no email configured at $ALVUM_EMAIL_FILE" >&2
  echo "set one:  echo you@example.com > $ALVUM_EMAIL_FILE" >&2
  exit 1
fi
RECIPIENT=$(tr -d '[:space:]' < "$ALVUM_EMAIL_FILE")

TODAY=$(today)
BRIEFING="$ALVUM_BRIEFINGS_DIR/$TODAY/briefing.md"
if [[ ! -f "$BRIEFING" ]]; then
  echo "no briefing for $TODAY. run briefing.sh first." >&2
  exit 1
fi

SUBJECT="alvum · $(date +"%A, %B %-d")"
mail -s "$SUBJECT" "$RECIPIENT" < "$BRIEFING"
echo "[$(now_utc)] emailed briefing to $RECIPIENT"
```

## `scripts/status.sh`

One-screen health report. Answers "is alvum working for me?"

```bash
#!/usr/bin/env bash
# Concise health report for this Mac's alvum install.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "alvum — $(date +'%Y-%m-%d %H:%M:%S')"
echo

# Binary.
if [[ -x "$ALVUM_BIN" ]]; then
  echo "  binary:     $ALVUM_BIN"
else
  echo "  binary:     MISSING ($ALVUM_BIN)"
fi

# Config.
if [[ -f "$ALVUM_CONFIG_FILE" ]]; then
  echo "  config:     $ALVUM_CONFIG_FILE"
else
  echo "  config:     MISSING"
fi

# Launchd jobs.
plist_loaded "$ALVUM_BRIEFING_LABEL" \
  && echo "  briefing:   scheduled" \
  || echo "  briefing:   not scheduled"
plist_loaded "$ALVUM_CAPTURE_LABEL" \
  && echo "  capture:    running" \
  || echo "  capture:    off"

# Today & yesterday briefings.
for d in "$(today)" "$(yesterday)"; do
  B="$ALVUM_BRIEFINGS_DIR/$d/briefing.md"
  if [[ -f "$B" ]]; then
    printf "  briefing %s: %d lines · modified %s\n" "$d" \
      "$(wc -l < "$B" | tr -d ' ')" "$(stat -f %Sm "$B")"
  else
    printf "  briefing %s: MISSING\n" "$d"
  fi
done

# Email config.
if [[ -f "$ALVUM_EMAIL_FILE" ]]; then
  echo "  email to:   $(tr -d '[:space:]' < "$ALVUM_EMAIL_FILE")"
else
  echo "  email to:   not configured"
fi

# Recent errors.
for lf in briefing.err capture.err; do
  path="$ALVUM_LOGS_DIR/$lf"
  if [[ -s "$path" ]]; then
    echo
    echo "  recent $lf (tail):"
    tail -5 "$path" | sed 's/^/    /'
  fi
done
```

## `scripts/view.sh`

```bash
#!/usr/bin/env bash
# Open today's briefing in the default viewer.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

B="$ALVUM_BRIEFINGS_DIR/$(today)/briefing.md"
if [[ ! -f "$B" ]]; then
  echo "no briefing for $(today). run:  $(dirname "$0")/briefing.sh"
  exit 1
fi
open "$B"
```

## `scripts/menu-bar.sh` — STOP-GAP SwiftBar plugin

**Governance rules (enforce these):**
- Total file size: **≤ 150 lines**. If you're tempted to add a settings panel or a configuration editor, write it in the real app instead.
- This script is deleted the day `alvum-app` ships with a native `NSStatusItem`. Nothing depends on it permanently.
- It only *views* state and invokes *existing scripts*. Never shells out to `alvum` directly — always go through `capture.sh` / `briefing.sh` / etc. so behavior stays consistent with CLI use.

```bash
#!/usr/bin/env bash
#
# ============================================================================
# STOP-GAP: SwiftBar menu-bar plugin for alvum.
# Delete this file when alvum-app ships its own NSStatusItem.
# Keep under 150 lines. No settings panels, no config editors — use CLI.
# ============================================================================
#
# <xbar.title>alvum</xbar.title>
# <xbar.version>0.1</xbar.version>
# <xbar.author>alvum</xbar.author>
# <xbar.desc>Status + quick actions for the alvum daily briefing.</xbar.desc>
# <swiftbar.hideRunInTerminal>true</swiftbar.hideRunInTerminal>
# <swiftbar.hideLastUpdated>true</swiftbar.hideLastUpdated>
# <swiftbar.hideAbout>true</swiftbar.hideAbout>

set -euo pipefail

# SwiftBar symlinks this script into its plugins dir; resolve the real path
# so we can source lib.sh and reach the other scripts.
REAL="$(readlink "$0" 2>/dev/null || echo "$0")"
SCRIPTS="$(cd "$(dirname "$REAL")" && pwd)"
source "$SCRIPTS/lib.sh"

# ---------- menu bar title (the single line visible on screen) ----------

if briefing_fresh_today; then
  if any_capture_stale; then
    echo "◐ alvum | color=orange size=13"
  else
    echo "● alvum | color=#4a9d3d size=13"
  fi
else
  echo "○ alvum | color=gray size=13"
fi

echo "---"

# ---------- briefing actions ----------

last=$(last_briefing_relative)
echo "Today's briefing ($last) | bash='$SCRIPTS/view.sh' terminal=false"
echo "Run briefing now | bash='$SCRIPTS/briefing.sh' terminal=true refresh=true"

echo "---"

# ---------- capture state + per-source toggles ----------

echo "Capture: $(capture_state) | disabled=true"

for src in claude-code audio-mic audio-system screen; do
  on=$(is_source_enabled "$src")
  marker=$([[ "$on" == "true" ]] && echo "●" || echo "○")
  echo "$marker  $src | bash='$SCRIPTS/capture.sh' param1=toggle param2=$src terminal=false refresh=true"
done

echo "---"

if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
  echo "Stop capture | bash='$SCRIPTS/capture.sh' param1=stop terminal=false refresh=true"
else
  echo "Start capture | bash='$SCRIPTS/capture.sh' param1=start terminal=false refresh=true"
fi

echo "---"

# ---------- diagnostics + file access ----------

echo "Status (verbose) | bash='$SCRIPTS/status.sh' terminal=true"
echo "Edit config | bash=/usr/bin/open param1='$ALVUM_CONFIG_FILE' terminal=false"
echo "Open briefings folder | bash=/usr/bin/open param1='$ALVUM_BRIEFINGS_DIR' terminal=false"
```

## `scripts/menu-bar-install.sh`

```bash
#!/usr/bin/env bash
# Install the stop-gap SwiftBar menu-bar plugin.
# Idempotent: re-running just re-symlinks.
#
# Requires Homebrew for SwiftBar install, unless SwiftBar is already present.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> menu-bar: install"

# 1. Ensure SwiftBar is installed.
if [[ ! -d /Applications/SwiftBar.app ]]; then
  if ! command -v brew >/dev/null; then
    cat >&2 <<EOF
SwiftBar is required for the menu-bar plugin, and Homebrew isn't installed.
Install SwiftBar manually from https://swiftbar.app/ and re-run this script.
EOF
    exit 1
  fi
  echo "--> installing SwiftBar via Homebrew"
  brew install --cask swiftbar
fi

# 2. Ensure SwiftBar's plugin dir exists.
PLUGIN_DIR="$HOME/Library/Application Support/SwiftBar/Plugins"
mkdir -p "$PLUGIN_DIR"

# 3. Symlink the plugin. SwiftBar reads the refresh interval from the filename:
#    alvum.60s.sh = run every 60 seconds.
chmod +x "$ALVUM_REPO/scripts/menu-bar.sh"
ln -sf "$ALVUM_REPO/scripts/menu-bar.sh" "$PLUGIN_DIR/alvum.60s.sh"
echo "    symlinked $ALVUM_REPO/scripts/menu-bar.sh -> $PLUGIN_DIR/alvum.60s.sh"

# 4. Start (or nudge) SwiftBar.
if ! pgrep -x SwiftBar >/dev/null; then
  open /Applications/SwiftBar.app
  echo "--> started SwiftBar"
  echo "    if it prompts for a plugin folder, choose: $PLUGIN_DIR"
else
  # Poke SwiftBar to rescan the plugin dir.
  osascript -e 'tell application "SwiftBar" to refresh all' 2>/dev/null || true
fi

echo
echo "menu-bar installed. look for the alvum dot in your menu bar (top right)."
echo "to remove:  $ALVUM_REPO/scripts/menu-bar-uninstall.sh"
```

## `scripts/menu-bar-uninstall.sh`

```bash
#!/usr/bin/env bash
# Remove the menu-bar plugin symlink. Does NOT uninstall SwiftBar itself.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

rm -f "$HOME/Library/Application Support/SwiftBar/Plugins/alvum.60s.sh"
osascript -e 'tell application "SwiftBar" to refresh all' 2>/dev/null || true
echo "menu-bar plugin removed. SwiftBar itself left installed."
```

## `scripts/uninstall.sh`

```bash
#!/usr/bin/env bash
# Clean reverse of install. Leaves ~/.alvum/generated/briefings/ alone unless --purge.
# Usage: uninstall.sh [--purge]

set -euo pipefail
source "$(dirname "$0")/lib.sh"

echo "==> uninstall"

# Remove the menu-bar plugin (if installed — silent if not).
"$(dirname "$0")/menu-bar-uninstall.sh" >/dev/null 2>&1 || true

# Stop capture first (if running).
if plist_loaded "$ALVUM_CAPTURE_LABEL"; then
  "$(dirname "$0")/capture.sh" stop
fi

# Remove the briefing schedule.
unload_plist "$ALVUM_LAUNCHAGENTS/$ALVUM_BRIEFING_LABEL.plist"

if [[ "${1:-}" == "--purge" ]]; then
  rm -rf "$ALVUM_ROOT"
  echo "purged ~/.alvum (config, briefings, capture, logs all under one root now)"
fi
echo "done."
```

---

## Launchd Plists

### `launchd/com.alvum.briefing.plist`

Daily at 07:00. Runs `briefing.sh`, which handles both extraction and (conditionally) email.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.alvum.briefing</string>

  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>@@ALVUM_REPO@@/scripts/briefing.sh</string>
  </array>

  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key><integer>7</integer>
    <key>Minute</key><integer>0</integer>
  </dict>

  <key>RunAtLoad</key>           <false/>
  <key>StandardOutPath</key>     <string>@@ALVUM_RUNTIME@@/logs/briefing.out</string>
  <key>StandardErrorPath</key>   <string>@@ALVUM_RUNTIME@@/logs/briefing.err</string>

  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
  </dict>
</dict>
</plist>
```

### `launchd/com.alvum.capture.plist`

Continuous. Loaded by `capture.sh start`, unloaded by `capture.sh stop`. The daemon reads `runtime/config.toml` at startup to decide which capture sources to run; per-source toggles flip the config and kickstart the daemon to reload.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.alvum.capture</string>

  <key>ProgramArguments</key>
  <array>
    <string>@@ALVUM_BIN@@</string>
    <string>capture</string>
  </array>

  <key>RunAtLoad</key>           <true/>
  <key>KeepAlive</key>           <true/>
  <key>ThrottleInterval</key>    <integer>10</integer>
  <key>StandardOutPath</key>     <string>@@ALVUM_RUNTIME@@/logs/capture.out</string>
  <key>StandardErrorPath</key>   <string>@@ALVUM_RUNTIME@@/logs/capture.err</string>

  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
  </dict>
</dict>
</plist>
```

---

## How the PMF-Path Stages Map to This Runbook

`pmf-path.md` describes three rollout stages. Each stage corresponds to running the same scripts with a different subset of capabilities enabled. Nothing about the scripts changes between stages.

| Stage (from `pmf-path.md`) | What's enabled | Commands the user runs |
|---|---|---|
| **Stage 0** — solo dogfood | Briefing scheduled; menu bar (default-installed unless declined); capture off; no email. | `./scripts/install.sh` |
| **Stage 1** — full multi-source | Briefing + capture running (claude + audio + screen) + menu bar; no email. | `./scripts/capture.sh start`, then `./scripts/capture.sh toggle audio-mic` / `toggle screen` as desired |
| **Stage 2** — 7 users + email | Everything above + email. | `echo you@example.com > ~/.alvum/runtime/email.txt` |

One install command; per-capability opt-in thereafter. To roll back any capability:
- Turn off a single source: `./scripts/capture.sh toggle <source>`
- Stop emailing: `rm ~/.alvum/runtime/email.txt`
- Stop capture entirely: `./scripts/capture.sh stop`
- Remove menu bar: `./scripts/menu-bar-uninstall.sh`
- Stop everything: `./scripts/uninstall.sh`

---

## Verification (per capability)

Each capability has its own check. These are the commands a user runs after enabling something to confirm it's working.

**After `install.sh`:**

```bash
./scripts/status.sh
# Expected: binary present, config present, briefing scheduled,
#           capture off, email not configured.

./scripts/briefing.sh
# Expected: runs to completion, creates briefings/<today>/briefing.md.

./scripts/view.sh
# Expected: today's briefing opens.
```

**After `capture.sh start`:**

```bash
./scripts/capture.sh status
# Expected: daemon loaded; per-source list matches config.

sleep 120
ls "$HOME/.alvum/capture/$(date +%Y-%m-%d)/"
# Expected: files have appeared under enabled sources' subdirectories.

tail "$HOME/.alvum/runtime/logs/capture.err"
# Expected: empty. If permission errors, grant via System Settings.
```

**After `capture.sh toggle <source>`:**

```bash
./scripts/capture.sh status
# Expected: the toggled source flipped in the list.

./scripts/capture.sh toggle screen    # example
./scripts/capture.sh toggle screen    # flip back
```

**After setting `~/.alvum/runtime/email.txt`:**

```bash
./scripts/briefing.sh
# Expected: briefing generated AND email sent (check your inbox).

./scripts/email.sh
# Expected: can also be invoked on its own to re-send today's briefing.
```

**After `menu-bar-install.sh`:**

```bash
# SwiftBar should now be running and an 'alvum' entry visible in the menu bar (top right).
pgrep -x SwiftBar
# Expected: a PID.

ls "$HOME/Library/Application Support/SwiftBar/Plugins/alvum.60s.sh"
# Expected: symlink to scripts/menu-bar.sh.

# Click the menu bar item. Expected menu:
#   - top line: "● alvum" (green when capture healthy; gray until first briefing)
#   - "Today's briefing (…)" — opens today's briefing
#   - "Run briefing now" — invokes briefing.sh
#   - Per-source toggle rows (●/○) — clicking flips that source
#   - Start/Stop capture, Status, Edit config, Open briefings folder
```

---

## Atomicity & Composition Rules

Rules every script obeys. These are what "reliable and easy to build off of" look like:

1. **One job per script.** Every script has a verb in its name (`install`, `briefing`, `capture`, `email`, `status`, `view`, `uninstall`, `menu-bar-install`, `menu-bar-uninstall`). `capture.sh` uses subcommands (`start`, `stop`, `status`, `toggle`) because those are the same capability from four directions.

2. **`lib.sh` is the only shared state.** Nothing else is sourced. No script reaches into another's file internals.

3. **Scripts compose via command invocation, never via sharing variables.** `briefing.sh` calls `email.sh` as a subprocess. No sourced chain-of-trust.

4. **`lib.sh` is the single source of paths.** `$ALVUM_ROOT`, `$ALVUM_CAPTURE`, `$ALVUM_GENERATED`, `$ALVUM_RUNTIME`, plist names, label names — all here. Changing a path is one edit.

5. **Config is the source of truth for what the pipeline does.** Launchd controls whether the daemon is *running*; `runtime/config.toml` controls *which sources* the running daemon captures. `capture.sh start` loads the daemon; `capture.sh toggle <source>` flips a config flag and kickstarts the daemon to reload. Both knobs are auditable: `alvum config-show` tells you what's enabled, `launchctl list | grep alvum` tells you what's loaded.

6. **Idempotency everywhere.** Re-running any script converges to the same state; never breaks the last-good state if interrupted.

7. **Templated plists.** Plists in `launchd/` contain `@@ALVUM_ROOT@@`, `@@ALVUM_RUNTIME@@`, `@@ALVUM_BIN@@`, `@@ALVUM_REPO@@`. `lib.sh::install_plist` resolves them at install time. No user-specific paths ever committed to the repo.

8. **Scripts never modify each other.** Adding a new capability means adding a new script (and maybe a plist). Never editing briefing.sh to "also do X." Extensions are additive.

9. **Uninstall is symmetric with install.** Whatever `install.sh` set up, `uninstall.sh` tears down. `--purge` goes further (wipes user data); it's explicit, never implicit.

10. **Debugging is always possible from `status.sh`.** If something's broken, that script reports enough (binary presence, config presence, plist state, last errors) to diagnose without reading code.

---

## Extending This Runbook

When a new capability needs to be added, follow this pattern (so future contributors don't need to re-derive the conventions):

1. **Pick a verb.** E.g., "send the weekly digest" → `digest.sh`. "Publish a public share link" → `share.sh`.
2. **Add one script** in `scripts/`, sourcing `lib.sh`.
3. **Add one plist** in `launchd/` if it needs to be scheduled or daemonized.
4. **Update `install.sh`** only if the capability is *always on* (rare — most should be opt-in).
5. **Update `status.sh`** to report the new capability's state (one added line).
6. **Add a row** to the PMF-Path stage mapping table above (if the new capability lands in a future stage).

Do not add a `stageN/` directory. Do not rename existing scripts. Do not repurpose old scripts with conditionals that depend on what stage the user is in.

**One exception: STOP-GAP scripts.** A script like `menu-bar.sh` is explicitly marked as stop-gap at the top of the file, sized-capped in a governance comment, and deleted wholesale when its replacement ships. Future stop-gaps follow the same pattern: header-commented as stop-gap with a defined end-of-life, size-capped, and only *compose* existing scripts (never reimplement their logic).

---

## Debugging Recipes

| Symptom | Likely cause | Command |
|---|---|---|
| No briefing at 07:00 | Plist not loaded | `launchctl list \| grep alvum.briefing` |
| Briefing is empty | No Claude Code activity in last 24h | `ls -lt ~/.claude/projects/*/*.jsonl \| head` |
| Briefing missing screen data | Capture daemon not loaded or config off | `./scripts/capture.sh status` |
| Capture daemon exits repeatedly | Missing macOS permissions | `tail ~/.alvum/runtime/logs/capture.err`; grant Screen Recording + Accessibility |
| Email never arrives | Recipient typo or Mail.app not configured | `cat ~/.alvum/runtime/email.txt`; `mail -s test $EMAIL < /etc/motd` |
| `cargo build` fails | Toolchain out of date | `rustup update stable` |
| `claude -p` hangs / errors | Not authenticated | `claude login` |
| Config change didn't take effect | Daemon caches settings | `./scripts/capture.sh stop && ./scripts/capture.sh start` |
| `briefing.sh` runs but output is stale | Old data in capture dir from yesterday's daemon run | `rm -rf ~/.alvum/capture/$(date -v-1d +%Y-%m-%d) && ./scripts/briefing.sh` |

---

## What Explicitly Stays Undone

Per `pmf-path.md`, the following are not scripted here and should not be added until after Stage 2 hits its signal:

- No audio scripts (mic, system audio — deferred to prevent transcription quality / privacy complexity).
- No wearable, CarPlay, iPhone, or Watch scripts.
- No Electron / iOS / Watch companion installers.
- No intention, alignment, evening check-in scripts.
- No web-UI launcher.

Any of these becoming needed means Stage 2 passed and we're into the Agency Layer / Phase B+ work. That runbook gets written then, not now.
