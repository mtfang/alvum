# AGENTS.md

Operational guide for agents iterating on Alvum. Focuses on **non-obvious
gotchas** — for the basic "what is this", read `docs/superpowers/specs/`.

## Repo at a glance

- Rust workspace under `crates/` — capture / processor / connector / pipeline / cli.
- Electron shell under `app/` — owns Mic + Screen Recording TCC grants and
  spawns the nested capture helper app's `alvum capture` as a child process.
- Mac-only. Runs on macOS Sequoia (Darwin 25+).

The running setup looks like this:

```
launchd
  └─ Alvum.app/Contents/MacOS/Alvum            ← Electron main, signed
       └─ Contents/Helpers/Alvum Capture.app/Contents/MacOS/alvum capture
          ← Rust subprocess, signed helper app with icon resources
```

The capture subprocess inherits TCC grants because Alvum.app is its
**responsible process**. Run `alvum` directly from a terminal and you
silently use the **terminal's** grants — every diagnostic test from the
terminal is on a different code path than the production one.

## Build / deploy

```bash
scripts/build-deploy.sh             # Rust-only iteration: rebuild → sign → reseal → relaunch UI
scripts/build-deploy.sh --full      # also npm run pack (after Electron / renderer / asset changes)
scripts/build-deploy.sh --start-capture  # relaunch and immediately resume capture
scripts/build-deploy.sh --no-restart
```

That's the only correct way to rebuild and redeploy. The script encodes
the multi-step sign + reseal + relaunch recipe — manual `cp` of a fresh
Rust binary into the bundle without re-signing breaks TCC silently.
Default relaunches write a one-shot launch intent under
`~/.alvum/runtime/launch-intent.json` so dev builds do not auto-start capture
and trip Mic / Screen permission surfaces. Use `--start-capture` for capture
iteration or permission verification.

Use `--full` after changes under `app/main.js`, `app/package.json`,
`app/popover.html`, `app/popover-preload.js`, `app/src/renderer/**`, or
`app/assets/**`.

## Popover renderer

The tray popover still loads through Electron with `app/main.js` owning
privileged work and `app/popover-preload.js` exposing the only renderer API.
`app/popover.html` is now just the shell/static markup. Renderer source lives
under `app/src/renderer/` and builds with esbuild into ignored generated files:

```text
app/renderer-dist/popover.js
app/renderer-dist/popover.css
```

Do not edit or commit `renderer-dist/`; run `cd app && npm run renderer:build`.
`npm test`, `npm run start`, `npm run pack`, and `npm run dist` all rebuild the
renderer first. `npm run renderer:check` runs the TypeScript no-emit check.

Browser preview is supported through the mock bridge:

```text
file:///.../app/popover.html?mock=idle
file:///.../app/popover.html?mock=capture
file:///.../app/popover.html?mock=briefing
file:///.../app/popover.html?mock=catchup
file:///.../app/popover.html?mock=update
```

The mock is a visual/interaction harness only. Final verification still needs
the packaged Electron popover because preload IPC, resize behavior, TCC state,
and tray lifecycle do not exist in a plain browser.

## Why each signing step exists

TCC validates **each process's own signing identity**, not the parent's
grant. If the inner Rust binary is ad-hoc-signed, its identifier looks
like `alvum-98dcc17612329072` (a content-hash) and changes on every
`cargo build` → TCC re-prompts every time. Three rules:

1. **Capture helper app + Rust binary**: sign with the configured identity via
   `scripts/sign-binary.sh` and `scripts/sign-app.sh`. Stable identifier
   (`com.alvum.capture`) plus real helper app resources gives System Settings
   a stable TCC row with the Alvum icon.
2. **Outer .app bundle**: sign with the same configured identity via
   `scripts/sign-app.sh`.
   This signs **inside-out** — every framework / helper / dylib first,
   then the outer bundle. Order matters because each parent's signature
   records its children's content hashes.
3. **No `--options runtime`** anywhere. Hardened runtime makes `dyld`
   refuse to load `Electron Framework.framework` with the cryptic
   `mapping process and mapped file (non-platform) have different
   Team IDs` error — even though `codesign --verify` says the bundle is
   valid. The Apr 23 working build had `flags=0x0(none)` and that's
   what we keep.

`electron-builder` can report `skipped macOS application code signing`.
That's expected — `sign-app.sh` does the actual signing afterwards.

## Cert provisioning

Signing uses a Developer ID Application certificate when one is installed.
Override with `ALVUM_SIGN_IDENTITY=<identity>`. If no Developer ID identity
exists, `scripts/sign-binary.sh` generates and reuses the self-signed
`alvum-dev` cert in the user's login keychain. This is what makes TCC grants
stable across rebuilds — TCC keys grants on the cert's designated requirement,
and we never re-issue the fallback cert.

The dev deploy path is Developer ID signed but not notarized. Notarization
requires hardened runtime, and this bundle still intentionally omits it.
`scripts/distribute-macos.sh` is the exception: it re-signs the built app with
hardened runtime for notarization and DMG creation. After a distribution dry run
against the default bundle, restore the local development bundle with
`scripts/build-deploy.sh --full` before continuing tray/capture iteration.

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

After `build-deploy.sh --start-capture` returns "done", confirm:

1. **Process tree** — capture subprocess parented under Alvum.app:
   ```
   ps -ax -o pid,ppid,command | awk '/Alvum.app|alvum capture/ && !/grep|awk/'
   ```
   You should see the Rust process's PPID match the Electron main's PID.
2. **No re-prompt** — `tail ~/.alvum/runtime/logs/shell.log` should show
   `[permissions] microphone status: granted` and `screen status: granted`
   immediately after launch. If it instead shows `Opening System Settings
   > Privacy & Security`, the inner binary's signing identity drifted.
3. **Capture helper identity** —
   `codesign -dv "$BUNDLE/Contents/Helpers/Alvum Capture.app/Contents/MacOS/alvum"`
   should report a real authority (`Developer ID Application: ...` or
   `alvum-dev`) and an identifier of the form `com.alvum.*`. If it says
   `Signature=adhoc` or an `alvum-<hex>` content-hash identifier, re-run
   `build-deploy.sh`.

## Notification icons (known limitation, do not relitigate)

Notifications render the alvum logo as the right-side ATTACHMENT image,
not the left-side SENDER icon. We accepted this — the brand is visible,
and chasing the sender slot is a rabbit hole.

Why: macOS Sequoia's `usernoted` resolves the sender icon by querying
LaunchServices for the calling bundle's icon, and self-signed apps without
an Apple-issued Team ID get a blank slot. We exhausted the workarounds
during 2026-04-25 — every cache-flush, helper-bundle icon-injection,
`CFBundleIconName` / `CFBundleIconFile` permutation, dock manipulation,
and `lsregister -f -R` came back the same. The bundle itself is correct
(`codesign --verify --deep --strict` is clean, `icon.icns` has all 10
sizes). It's a macOS-side gate.

The only verified fixes are:

1. Apple Developer ID signing ($99/yr) — flips it.
2. Compile `Assets.car` via Xcode `actool` (full Xcode required, not
   Command Line Tools) and ship next to `icon.icns`.

Both are out of scope until there's a real reason. Until then the
attachment surface is enough.

The icon comes from `app/assets/icon.png` (1024×1024 master, RGBA on
white). `main.js` caches it as `APP_ICON = nativeImage.createFromPath(...)`
and passes it to every `new Notification({...icon: APP_ICON})` call.
Out-of-process callers go through the queue at
`~/.alvum/runtime/notify.queue` (helper `alvum_notify` in `lib.sh`); the
running Alvum.app polls every 500 ms and emits via the same path so all
toasts get the brand attachment.

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

## Debugging the briefing pipeline

The pipeline emits two parallel streams of observability data:

```
~/.alvum/runtime/briefing.progress    # narrow: stage + bar progress
~/.alvum/runtime/pipeline.events      # rich: stage / LLM / inventory / filter events
```

Both are append-only JSONL files, truncated at run-start.
`pipeline.events` is the one to watch when something feels off.

```bash
# Stream the live event log to stderr (companion to the popover panel).
alvum tail --follow

# Filter to a single concern.
alvum tail --follow --filter llm_call      # only LLM round-trips
alvum tail --follow --filter stage         # only stage transitions
alvum tail --follow --filter input_filter  # only filter outcomes
alvum tail --follow --filter warning       # only warnings + errors
```

What each event class tells you:

- `stage_enter` / `stage_exit` — pipeline lifecycle. Compare `elapsed_ms`
  values to spot regressions.
- `input_inventory` — per-source ref count from gather. Zero counts on
  declared `expected_sources` indicate a silent modality.
- `llm_call_start` / `llm_call_end` — every observed LLM round-trip
  (`thread/chunk_N`, `distill`, `causal`, `brief`, `knowledge`,
  `vision/...`). The `latency_ms` field is the primary cost indicator.
- `llm_parse_failed` — paired with a retry call to `<call_site>/retry`.
  Counts of these are the canonical "Claude is hallucinating
  conversationally" diagnostic.
- `input_filtered` — drop counts and reasons per processor (whisper
  `no_speech_prob` vs `low_token_prob`, knowledge schema validations,
  causal forward-references, etc.).
- `warning` / `error` — soft and hard signals respectively. The tray
  popover surfaces these at the top of the live panel.

The same stream feeds the popover's "Live pipeline" panel — they are
two views of the same JSONL file, so the GUI and the terminal never
disagree.
