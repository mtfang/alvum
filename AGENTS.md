# AGENTS.md

Operational guide for agents iterating on Alvum. Focuses on **non-obvious
gotchas** — for the basic "what is this", read `docs/superpowers/specs/`.

## Repo at a glance

- Rust workspace under `crates/` — capture / processor / connector / pipeline / cli.
- Electron shell under `app/` — owns Mic + Screen Recording TCC grants and
  spawns `bin/alvum capture` as a child process.
- Mac-only. Runs on macOS Sequoia (Darwin 25+).

The running setup looks like this:

```
launchd
  └─ Alvum.app/Contents/MacOS/Alvum            ← Electron main, alvum-dev signed
       └─ Contents/Resources/bin/alvum capture ← Rust subprocess, alvum-dev signed
```

The capture subprocess inherits TCC grants because Alvum.app is its
**responsible process**. Run `bin/alvum` directly from a terminal and you
silently use the **terminal's** grants — every diagnostic test from the
terminal is on a different code path than the production one.

## Build / deploy

```bash
scripts/build-deploy.sh             # Rust-only iteration: rebuild → sign → reseal → relaunch
scripts/build-deploy.sh --full      # also npm run pack (after main.js / assets / package.json changes)
scripts/build-deploy.sh --no-restart
```

That's the only correct way to rebuild and redeploy. The script encodes
the multi-step sign + reseal + relaunch recipe — manual `cp` of a fresh
Rust binary into the bundle without re-signing breaks TCC silently.

## Why each signing step exists

TCC validates **each process's own signing identity**, not the parent's
grant. If the inner Rust binary is ad-hoc-signed, its identifier looks
like `alvum-98dcc17612329072` (a content-hash) and changes on every
`cargo build` → TCC re-prompts every time. Three rules:

1. **Inner Rust binary**: sign with `alvum-dev` cert via
   `scripts/sign-binary.sh`. Stable identifier (`com.alvum.cli`) →
   stable TCC grants across rebuilds.
2. **Outer .app bundle**: sign with `alvum-dev` via `scripts/sign-app.sh`.
   This signs **inside-out** — every framework / helper / dylib first,
   then the outer bundle. Order matters because each parent's signature
   records its children's content hashes.
3. **No `--options runtime`** anywhere. Hardened runtime makes `dyld`
   refuse to load `Electron Framework.framework` with the cryptic
   `mapping process and mapped file (non-platform) have different
   Team IDs` error — even though `codesign --verify` says the bundle is
   valid. The Apr 23 working build had `flags=0x0(none)` and that's
   what we keep.

`electron-builder` reports `skipped macOS application code signing`
because `alvum-dev` is self-signed (`CSSMERR_TP_NOT_TRUSTED`). That's
expected — `sign-app.sh` does the actual signing afterwards.

## Cert provisioning

`alvum-dev` is a self-signed code-signing cert in the user's login
keychain. First `scripts/sign-binary.sh` run generates it. Subsequent
runs reuse it. This is what makes TCC grants stable across rebuilds —
TCC keys grants on the cert's designated requirement, and we never
re-issue the cert.

## Live-bundle path

The Mac dock alias is pinned to the **main worktree's** bundle:

```
~/git/alvum/app/dist/mac-arm64/Alvum.app
```

`build-deploy.sh` defaults to the bundle inside whichever repo the
script lives in (`$ALVUM_REPO/app/dist/mac-arm64/Alvum.app`). When
iterating from a worktree, that means `build-deploy.sh` deploys to the
**worktree's own bundle** — the dock alias will still point at the
old main-path bundle. Two ways to handle this:

```bash
# Iterate quickly — deploys to the worktree's bundle, which the script
# also relaunches. Dock alias is stale during this phase.
scripts/build-deploy.sh

# When ready to update the dock-pinned bundle:
scripts/build-deploy.sh --bundle ~/git/alvum/app/dist/mac-arm64/Alvum.app
```

After PR merge, a normal `npm run pack` from the main worktree puts
everything back at the canonical path automatically.

## Verify after deploy

After `build-deploy.sh` returns "done", confirm:

1. **Process tree** — capture subprocess parented under Alvum.app:
   ```
   ps -ax -o pid,ppid,command | awk '/Alvum.app|alvum capture/ && !/grep|awk/'
   ```
   You should see the Rust process's PPID match the Electron main's PID.
2. **No re-prompt** — `tail ~/.alvum/runtime/logs/shell.log` should show
   `[permissions] microphone status: granted` and `screen status: granted`
   immediately after launch. If it instead shows `Opening System Settings
   > Privacy & Security`, the inner binary's signing identity drifted.
3. **Inner binary identity** — `codesign -dv $BUNDLE/Contents/Resources/bin/alvum`
   should report `Authority=alvum-dev`, `Signature size=1666` (real cert),
   and an identifier of the form `com.alvum.*`. If it says `Signature=adhoc`
   or an `alvum-<hex>` content-hash identifier, re-run `build-deploy.sh`.

## Audio capture knobs

Both gates live in `~/.alvum/runtime/config.toml`:

```toml
[capture.audio-mic]
silence_threshold_dbfs = -45    # window-RMS floor; raise to be stricter
silence_hold_secs      = 2.0    # ±halo around any passing window

[capture.audio-system]
silence_threshold_dbfs = -63
silence_hold_secs      = 2.0
```

The gate runs at 20-ms window granularity with a hold-time halo (see
`crates/alvum-capture-audio/src/encoder.rs`). Without the halo, speech
gets chopped between syllables and sounds fast-forwarded; without the
threshold, idle stretches and digital silence fill disk for nothing.

Mic-side device selection follows the macOS default among non-Bluetooth
inputs (logic in `crates/alvum-capture-audio/src/mic_selection.rs`),
re-polling every 3 s. Override with:

```toml
[capture.audio-mic]
mic_device = "Studio Display Microphone"
```

## When something silently breaks

Most "the capture isn't running" symptoms trace back to one of:

1. **Inner binary lost stable signing identity.** Symptom: capture.out
   logs `Screen Recording permission not granted (failed to obtain
   SCShareableContent)` and System Settings opens. Fix: `build-deploy.sh`.
2. **Bundle's sealed-resources hash invalidated.** Symptom: app refuses
   to launch (no shell.log entry on `open Alvum.app`). Fix:
   `build-deploy.sh --no-restart` then `open` manually.
3. **Hardened runtime accidentally re-introduced.** Symptom: dyld error
   on launch about Team IDs and Electron Framework. Fix: re-run
   `sign-app.sh` (it always omits `--options runtime`).
4. **Running from terminal, not the .app.** Symptom: TCC dialog appears
   but everything else looks fine. Fix: `pkill` direct invocations and
   relaunch through `open Alvum.app`.
