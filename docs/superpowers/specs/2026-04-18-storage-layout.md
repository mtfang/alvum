# Storage Layout (Authoritative)

All alvum on-disk state lives under **`~/.alvum/`**, organized by **lifecycle** — not by data source, not by component, not by rollout stage. This spec supersedes every earlier path reference in this repo (top-level spec § Storage, Phase A `AlvumPaths`, the PMF runbook's `paths.sh`). Those documents are updated to match this spec; if they disagree, this one wins.

## Decision

Single root: `~/.alvum/`.

Three top-level lifecycle buckets beneath it:

1. **`capture/`** — **the ground truth.** Raw ingest. Kept indefinitely. Back this up. The most valuable data in the system: as models and embeddings improve, re-running the pipeline against old raw capture produces a materially better `generated/` — something you cannot do if you've thrown the raw away. A year of raw audio from 2026 is worth more in 2028 than it is today.
2. **`generated/`** — **the current derivation.** Everything the pipeline has extracted so far: decision graph, knowledge corpus, day extractions, briefings, plus user-stated state (intentions, check-in responses). Back this up — it's cheap, small, and some of it (user-stated state, user-confirmed entities) cannot be re-derived. Regenerable from `capture/` + a better model for everything else.
3. **`runtime/`** — operational state. Binary, logs, tokens, indexes, caches. Truly regenerable. Never back up.

Plus a `VERSION` file at the root for data-layout migration.

```
~/.alvum/
├── VERSION                     # integer; bumped on any incompatible layout change
├── generated/                       # CURRENT DERIVATION — back up (small, partly re-derivable, partly user-stated)
│   ├── decisions/
│   │   ├── index.jsonl
│   │   ├── open.jsonl
│   │   └── states.jsonl
│   ├── knowledge/
│   │   ├── entities.jsonl
│   │   ├── patterns.jsonl
│   │   └── facts.jsonl
│   ├── days/
│   │   └── YYYY-MM-DD.json
│   ├── episodes/
│   │   └── YYYY-MM-DD/
│   │       ├── threads.json
│   │       └── time_blocks.json
│   ├── briefings/
│   │   └── YYYY-MM-DD/
│   │       ├── briefing.md
│   │       ├── decisions.jsonl      # snapshot from this run (debug trail)
│   │       └── threads.json         # snapshot from this run
│   ├── checkins/
│   │   └── YYYY-MM-DD/
│   │       ├── questions.json
│   │       └── responses.jsonl
│   ├── intentions.json
│   └── life_phase.json              # empty until Agency Layer ships; file slot reserved
├── capture/                    # GROUND TRUTH — kept indefinitely; back up (the raw is your re-extraction fuel)
│   └── YYYY-MM-DD/
│       ├── audio/
│       │   ├── mic/
│       │   ├── system/
│       │   └── wearable/
│       ├── screen/
│       │   ├── events.jsonl
│       │   └── snapshots/
│       ├── location/
│       │   ├── raw/
│       │   │   ├── iphone.jsonl
│       │   │   ├── mac.jsonl
│       │   │   └── wearable.jsonl
│       │   ├── pulled/
│       │   │   ├── strava.json
│       │   │   └── healthkit.json
│       │   └── fused.jsonl
│       ├── health/
│       │   ├── hr.jsonl
│       │   ├── activity.jsonl
│       │   ├── sleep.jsonl
│       │   ├── workouts.jsonl
│       │   └── mindful.jsonl
│       ├── frames/                   # wearable camera frames
│       └── location.jsonl            # legacy flat fallback; phased out with location/ tree
└── runtime/                    # OPERATIONAL — regenerable, ephemeral
    ├── bin/
    │   └── alvum                     # the installed binary (chmod +x)
    ├── config.toml                   # the only config file; edited by `alvum config-set`
    ├── email.txt                     # optional; present → briefing.sh triggers email.sh
    ├── devices/
    │   ├── registry.json             # device-fleet state (Phase C+)
    │   ├── tokens.json               # bearer tokens per device (0600)
    │   └── heartbeats/
    │       └── <device_id>.jsonl     # 14-day retention
    ├── embeddings/
    │   ├── decisions.idx
    │   ├── observations.idx
    │   └── media.idx                 # populated at V2/V3; file slot reserved
    ├── cache/
    │   └── geocode/
    │       └── h3-r9.jsonl           # reverse-geocode cache (30-day TTL)
    └── logs/
        ├── briefing.out
        ├── briefing.err
        ├── capture.out
        └── capture.err
```

## Why This Over the Alternatives

Three alternatives considered. All rejected for reasons specific to the goals "scalable" and "organized":

- **`~/Library/Application Support/com.alvum.app/`** (the top-level spec's original choice). Apple-canonical but opaque to users (`cd` into it is awkward), mixes hot and cold data under one flat tree, and fights against the user's explicit preference for something home-rooted. Kept only for the config file? No — one-root discipline is worth breaking the Apple convention. Config is small; one directory tree wins.
- **`~/alvum/`** (visible home directory). Easy to discover but pollutes home. `ls ~` shouldn't grow a tool's directory.
- **Flat `~/.alvum/{capture, days, decisions, ...}/`** (top-level spec's layout under a different root). Scales poorly — no boundary between "the decision graph I'd cry over if lost" and "log files I can wipe any time." The lifecycle grouping turns that boundary into a directory.

The chosen design gives:
- One root for `cd ~/.alvum`.
- One subtree to back up (`generated/`) — a 30-second rsync to a NAS.
- One subtree to retention-prune (`capture/`) — a cron job that can't accidentally delete the asset.
- One subtree to wipe when debugging (`runtime/`) — regenerable on next run.
- Explicit extension points (`generated/<new-type>/`, `capture/YYYY-MM-DD/<new-source>/`, `runtime/<new-component>/`) — adding a spec doesn't require new top-level directories or rethinking lifecycle.

## Backup & Retention Contract

The directory structure encodes the lifecycle. The backup rule is simple: **back up everything except `runtime/`.**

```bash
# Nightly backup. One rsync, one line.
rsync -av --delete --exclude runtime/ ~/.alvum/ nas:alvum-backup/
```

Restore is symmetric: rsync back, reinstall the binary, done.

### Retention

**`capture/` default: keep indefinitely.** This is deliberate. The tradeoff — ~100 GB/year of audio + frames + events vs. a materially better `generated/` after every model upgrade — favors keeping. A 2TB external drive covers ~20 years. Users who disagree can set a pruning policy in `runtime/config.toml`:

```toml
[retention.capture]
# Drop capture dirs older than this many days. 0 = never prune.
max_age_days = 0

# Keep raw files linked from any Decision.evidence even if older than max_age_days.
keep_linked_evidence = true

# Per-source overrides (if disk-pressured, e.g., audio is large).
[retention.capture.audio]
max_age_days = 365
```

The pruner (a nightly script, not yet implemented) scans `capture/` only. It never touches `generated/` or `runtime/`. If pointed elsewhere, it's a bug.

**`generated/` default: keep forever.** It's small (a few GB at most over years). Some of it is re-derivable from capture; some is user-stated and irreplaceable. Retention-prune: never, by design. The only way data leaves `generated/` is explicit user action (delete a specific decision, forget a specific device).

**`runtime/` default: keep as long as useful.** Logs are capped (launchd rotation) or pruned by their owning subsystem (14-day heartbeats per the device-fleet spec). No retention config needed.

### Re-extraction

**A future `alvum reprocess` command** (not yet implemented) reads `capture/<date>/` with the current pipeline and rewrites `generated/` entries for that day. Use cases:

- A better model ships → re-run against the last 12 months of audio for better decisions and richer causal links.
- A new extraction capability lands (emotional tone, domain-specific entity extraction) → backfill it across historical capture.
- A prompt improvement fixes a systematic extraction bug → regenerate the affected date ranges.

**User-stated state in `generated/` is not touched by re-extraction.** `intentions.json`, confirmed `knowledge/facts.jsonl` entries, check-in responses, life-phase declarations — these are ground truth too, and re-extraction only rewrites the LLM-derived subset. The split is load-bearing; this is why the knowledge corpus needs to track the distinction between extracted and user-confirmed entries (a follow-up concern for `alvum-knowledge`).

### Wipe

- `rm -rf ~/.alvum/runtime/` — always safe. Regenerable.
- `rm -rf ~/.alvum/generated/` — destroys current derivation. Still recoverable by re-running the pipeline against `capture/`, provided a backup of `capture/` exists and the user-stated files were backed up separately.
- `rm -rf ~/.alvum/capture/` — destroys the ground truth. `generated/` still works but no future re-extraction is possible against pre-wipe time ranges.
- `rm -rf ~/.alvum/` — clean uninstall. `uninstall.sh --purge` does this.

## Extension Rules

When new data lands, pick the lifecycle and place accordingly:

| New thing | Place it in | Examples |
|---|---|---|
| A new kind of **refined, derived data the user would care about losing** | `generated/<name>/` | `generated/commitments/`, `generated/alignment_reports/` |
| A new **raw source or sensor stream** | `capture/YYYY-MM-DD/<source>/` | `capture/.../bio/`, `capture/.../calendar/` |
| **Operational state** (tokens, caches, indexes, logs) | `runtime/<component>/` | `runtime/sessions/`, `runtime/api-keys.json` |

Never add new top-level siblings to `generated/`, `capture/`, `runtime/`. Three is enough forever.

Per-day subdirectories (`generated/briefings/YYYY-MM-DD/`, `capture/YYYY-MM-DD/`, `generated/episodes/YYYY-MM-DD/`) use ISO 8601 dates (`%Y-%m-%d`). One format everywhere — no `04-18-2026`, no `20260418`, no `04_18`.

JSONL files are append-only. JSON files are rewritten atomically (`write to .tmp, fsync, rename`).

## Version File

```
~/.alvum/VERSION
```

Single line: a positive integer. Current value: `1`. Bumped when any incompatible layout change ships (a rename, a split, a removal of an existing directory). The binary reads this at startup and refuses to run against a mismatched VERSION — prompts the user to either upgrade alvum or run a migration script.

Migration scripts live in `scripts/migrate/` (added when the first migration is needed; not needed yet).

## Code Impact

### `alvum-core::paths::AlvumPaths`

The Phase A plan (`2026-04-18-alignment-primitives.md`) defines `AlvumPaths::default_root()` using `dirs::data_dir().join(APP_ID)`. This changes to:

```rust
pub fn default_root() -> Result<Self> {
    let root = dirs::home_dir()
        .context("could not determine home directory")?
        .join(".alvum");
    Ok(Self { root })
}
```

`APP_ID` is no longer used for root resolution; it stays as a constant for any future identifier needs (bundle ID, UserAgent strings) but doesn't appear in paths.

All subpath helpers are rebased to the three-bucket layout. Concrete mapping:

| Old helper | New path |
|---|---|
| `capture_dir(date)` | `capture/YYYY-MM-DD/` (unchanged) |
| `day_file(date)` | `generated/days/YYYY-MM-DD.json` |
| `decisions_index()` | `generated/decisions/index.jsonl` |
| `decisions_open()` | `generated/decisions/open.jsonl` |
| `decisions_states()` | `generated/decisions/states.jsonl` |
| `knowledge_entities()` | `generated/knowledge/entities.jsonl` |
| `knowledge_patterns()` | `generated/knowledge/patterns.jsonl` |
| `knowledge_facts()` | `generated/knowledge/facts.jsonl` |
| `episodes_dir(date)` | `generated/episodes/YYYY-MM-DD/` |
| `threads_file(date)` | `generated/episodes/YYYY-MM-DD/threads.json` |
| `time_blocks_file(date)` | `generated/episodes/YYYY-MM-DD/time_blocks.json` |
| `intentions_file()` | `generated/intentions.json` |
| `briefing_file(date)` | `generated/briefings/YYYY-MM-DD/briefing.md` |
| `checkin_questions_file(date)` | `generated/checkins/YYYY-MM-DD/questions.json` |
| *new* `life_phase_file()` | `generated/life_phase.json` |
| *new* `bin_dir()` | `runtime/bin/` |
| *new* `config_file()` | `runtime/config.toml` |
| *new* `email_file()` | `runtime/email.txt` |
| *new* `log_file(name)` | `runtime/logs/<name>` |
| *new* `devices_registry()` | `runtime/devices/registry.json` |
| *new* `devices_tokens()` | `runtime/devices/tokens.json` |
| *new* `device_heartbeats(id)` | `runtime/devices/heartbeats/<id>.jsonl` |
| *new* `embeddings_file(name)` | `runtime/embeddings/<name>.idx` |
| *new* `geocode_cache()` | `runtime/cache/geocode/h3-r9.jsonl` |
| `location_raw_file(date, src)` | `capture/YYYY-MM-DD/location/raw/<src>.jsonl` (unchanged) |
| `location_fused_file(date)` | `capture/YYYY-MM-DD/location/fused.jsonl` (unchanged) |
| `health_hr_file(date)` etc. | `capture/YYYY-MM-DD/health/<category>.jsonl` (unchanged) |

The Phase A plan's Task 1 adds `generated_dir()`, `capture_root()`, `runtime_dir()` as bucket-level helpers for consistency. Every method above is composed from these.

### `alvum-core::config`

`config_path()` changes:

```rust
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("config.toml")
}
```

No more `dirs::config_dir()`. Config is data now, lives with data.

`default_output_dir()` changes to return `AlvumPaths::default_root().generated_dir().join("briefings")` — the pipeline writes briefings, so its default output points at where briefings live.

### PMF Runbook (`2026-04-18-pmf-runbook.md`)

`scripts/lib.sh` conventions change to:

```bash
export ALVUM_ROOT="${ALVUM_ROOT:-$HOME/.alvum}"
export ALVUM_GENERATED="$ALVUM_ROOT/generated"
export ALVUM_CAPTURE_DIR="$ALVUM_ROOT/capture"
export ALVUM_RUNTIME="$ALVUM_ROOT/runtime"

export ALVUM_BIN="$ALVUM_RUNTIME/bin/alvum"
export ALVUM_BRIEFINGS_DIR="$ALVUM_GENERATED/briefings"
export ALVUM_LOGS_DIR="$ALVUM_RUNTIME/logs"
export ALVUM_EMAIL_FILE="$ALVUM_RUNTIME/email.txt"

export ALVUM_CONFIG_FILE="$ALVUM_RUNTIME/config.toml"
```

Old references to `$HOME/alvum/...` (no dot, single level) are replaced 1:1 with the above. Scripts that referenced `$ALVUM_CONFIG_DIR` separately just use `$ALVUM_RUNTIME` now.

`install.sh` writes:
- binary to `$ALVUM_BIN` (`~/.alvum/runtime/bin/alvum`)
- config to `$ALVUM_CONFIG_FILE` (`~/.alvum/runtime/config.toml`)
- ensures `$ALVUM_GENERATED` and `$ALVUM_CAPTURE_DIR` exist

The `ensure_dirs()` helper creates: `~/.alvum/{generated,capture,runtime/{bin,logs},runtime/cache,runtime/devices}`.

Everything else in the runbook composes from the new `lib.sh`.

### Top-level spec (§ Storage)

Update the existing § Storage section to reference this spec. Keep the three-bucket explanation but prune the per-file tree in the top-level spec (it rots) — this spec owns the tree.

## Migration From Current State

Current state of a dev box that's been running the in-repo extractor:

```
alvum/                              ← the cloned repo
├── capture/                        ← gitignored but in-repo
├── output/                         ← gitignored but in-repo
│   └── <ad-hoc decision files>
└── ...
```

One-time migration script (`scripts/migrate/v0-to-v1.sh`, written once needed):

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../lib.sh"

mkdir -p "$ALVUM_GENERATED/decisions" "$ALVUM_GENERATED/briefings" "$ALVUM_CAPTURE_DIR" "$ALVUM_RUNTIME"

# Move any prior outputs.
if [[ -d "$ALVUM_REPO/output" ]]; then
  rsync -a --remove-source-files "$ALVUM_REPO/output/" "$ALVUM_GENERATED/briefings/"
  rmdir -p "$ALVUM_REPO/output" 2>/dev/null || true
fi

if [[ -d "$ALVUM_REPO/capture" ]]; then
  rsync -a --remove-source-files "$ALVUM_REPO/capture/" "$ALVUM_CAPTURE_DIR/"
  rmdir -p "$ALVUM_REPO/capture" 2>/dev/null || true
fi

echo 1 > "$ALVUM_ROOT/VERSION"
echo "migrated to v1 layout at $ALVUM_ROOT"
```

Run this exactly once when the user first pulls the layout change. The `.gitignore` entries for `capture/` and `output/` in the repo can be removed — those directories should not appear in the repo again.

## What This Does Not Cover

Out of scope for this decision:

- **Managed-tier cloud sync layout.** When the managed tier ships (V1+), encrypted blobs go somewhere else entirely — likely an opaque remote blob store keyed by user id. This spec owns local; managed-tier gets its own spec.
- **Multi-user on one Mac.** Each macOS user account has its own `~/.alvum/`. Shared Box scenarios (a family device) are a future problem; not a V1 concern.
- **Sync across devices.** V3.5 in the growth path uses Syncthing to replicate across a user's machines. When that ships, it syncs `generated/` only by default; this spec's backup rule already encodes that.
- **Encrypted-at-rest for local data.** The spec is plaintext-on-disk for V1. FileVault is the user's responsibility. Local encryption of `generated/` specifically (at-rest encryption with Keychain-wrapped keys) is a future addition — slot in `runtime/keys/` when needed.

## Commit Checklist for Rolling Out This Spec

Seven changes across the repo; each is small and independent:

1. ✅ Write this spec (done by this doc).
2. Update `alvum-core::paths::AlvumPaths::default_root()` to the new single-root logic. Rebase every subpath helper onto `generated_dir()` / `capture_root()` / `runtime_dir()`.
3. Update `alvum-core::config::config_path()` to point at `~/.alvum/runtime/config.toml`.
4. Update `alvum-core::config::default_output_dir()` to point at `~/.alvum/generated/briefings/`.
5. Update the Phase A plan (`2026-04-18-alignment-primitives.md`) — rewrite Task 1's `AlvumPaths` module code block to match this spec. The test names stay the same; only paths change.
6. Update the PMF runbook (`2026-04-18-pmf-runbook.md`) — rewrite `scripts/lib.sh` to export the three-bucket vars, and update `install.sh` / `ensure_dirs` / any script that hardcoded a path.
7. Update the top-level spec § Storage to point here.

After step 2 lands, `cargo test --workspace` should still pass — only paths change, not shapes.
