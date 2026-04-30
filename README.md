# Alvum

Alvum is a macOS-first alignment engine. It captures local activity, processes
it overnight, builds a source-grounded decision graph, and writes a daily
briefing that focuses on gaps between stated intentions and observed behavior.

## Current State

The repo is a Rust workspace plus an Electron menubar shell:

- `crates/alvum-core` owns shared contracts: config, `DataRef`,
  `Observation`, `Connector`, `CaptureSource`, `Processor`, `Decision`, `Edge`,
  LLM provider traits, pipeline event/progress files, and virtual built-in
  component manifests for core audio, screen, and session components.
- `crates/alvum-cli` is the installed `alvum` binary. It runs capture,
  extraction, provider checks, config edits, event tails, and extension package
  management.
- `crates/alvum-pipeline` owns the briefing pipeline:
  `Observation -> TimeBlock -> Thread -> Cluster -> Domain Decisions ->
  Decision Edges -> Day Briefing -> Knowledge`.
- `crates/alvum-connector-*` are first-party connectors for audio, screen,
  Claude Code sessions, Codex sessions, and external HTTP extensions.
- `crates/alvum-capture-*` and `crates/alvum-processor-*` are reusable capture
  and processing primitives.
- `app/` is the Electron menubar app. It owns macOS TCC permission flow and
  spawns the signed Rust binary from the bundled app.

The production process tree is:

```text
launchd
  -> Alvum.app/Contents/MacOS/Alvum
       -> Contents/Helpers/Alvum Capture.app/Contents/MacOS/alvum capture
       -> scripts/briefing.sh
            -> alvum extract
```

Running `alvum` directly from a terminal is a different TCC path from the
production helper app. For capture/signing work, follow `AGENTS.md`.

## Build And Deploy

Use the project script:

```bash
scripts/build-deploy.sh
scripts/build-deploy.sh --full
scripts/build-deploy.sh --no-restart
```

The script rebuilds, signs the inner Rust binary and Electron bundle with a
stable code-signing identity, reseals the Electron bundle, and relaunches.
If a `Developer ID Application` certificate is installed, it is used by
default; otherwise the scripts fall back to the local `alvum-dev` identity.
Override with `ALVUM_SIGN_IDENTITY=<identity>`. Manual binary copies into the
app bundle can silently break macOS permissions.

This local deploy path signs with Developer ID but does not notarize. The app
still intentionally omits hardened runtime because the current Electron bundle
does not launch reliably with it enabled.

For a notarized release artifact:

```bash
export ALVUM_NOTARY_PROFILE=alvum-notary
./scripts/distribute-macos.sh
```

Distribution notarization re-signs the built app with hardened runtime just before
DMG packaging. Local `build-deploy` behavior is unchanged to avoid TCC regressions.
If you need a locally launchable unsigned-notary debug artifact, pass
`--skip-hardened-sign`.

The notary profile is a local keychain profile created with `xcrun notarytool`:

```bash
xcrun notarytool store-credentials alvum-notary \
  --apple-id "you@example.com" \
  --team-id F7LD227J88 \
  --password "@env:ALVUM_APP_SPECIFIC_PASSWORD"
```

`distribute-macos.sh` runs `build-deploy.sh --full --no-restart`, verifies
that a non-fallback Developer ID identity is available, writes the updater feed
config, notarizes and staples the app bundle for auto-update, builds the
updater ZIP plus `latest-mac.yml`, then signs/notarizes/staples the drag-install
DMG. Artifacts land in `app/dist/release/`:

- `Alvum-<version>-arm64.dmg` for users to drag into `/Applications`
- `Alvum-<version>-arm64-mac.zip` for the in-app updater
- `latest-mac.yml` for the GitHub Releases update feed
- `.sha256` checksums for the DMG and updater ZIP

Useful verification:

```bash
cargo test --workspace
ps -ax -o pid,ppid,command | awk '/Alvum.app|alvum capture/ && !/grep|awk/'
alvum tail --follow --filter stage
alvum tail --follow --filter llm_call
```

## Storage

All runtime state lives under `~/.alvum/`:

- `~/.alvum/capture/` is raw capture and should be retained.
- `~/.alvum/generated/` is derived state: briefings, decisions, graph snapshots,
  and knowledge.
- `~/.alvum/runtime/` is operational state: config, logs, binaries, caches, and
  installed extensions.

Daily briefing outputs currently land under:

```text
~/.alvum/generated/briefings/YYYY-MM-DD/
  briefing.md
  decisions.jsonl
  transcript.jsonl
  tree/L3-clusters.jsonl
  tree/L3-edges.jsonl
  tree/L4-domains.jsonl
  tree/L4-edges.jsonl
  tree/L5-day.json
  knowledge/
  extensions/
```

`tree/L4-edges.jsonl` is the rich decision graph edge artifact.
`decisions.jsonl` is the node snapshot plus compatibility `causes/effects`
projection.

## Connectors And Extensions

First-party connectors still implement Rust traits in-process. External
extensions are HTTP services managed by Alvum:

- A package declares capture, processor, analysis, and connector components in
  `alvum.extension.json`.
- A connector is the user-facing bundle. It owns the default routing matrix
  from capture components to processor components.
- Routes use global component IDs, so connectors can compose components from
  other installed and enabled packages.
- Capture components produce `DataRef`s.
- Processor components consume `DataRef`s and emit final `Observation`s.
- Analysis lenses consume brokered Alvum context and produce custom generated
  artifacts or graph overlays.

Installed packages live in:

```text
~/.alvum/runtime/extensions/
```

Extension commands:

```bash
alvum extensions scaffold ./my-extension --id my_extension --name "My Extension"
alvum extensions install /path/to/package
alvum extensions install git:https://example.com/repo.git
alvum extensions install npm:package-name
alvum extensions enable <package-id>
alvum extensions list
alvum extensions list --json
alvum extensions doctor
alvum extensions doctor --json
alvum extensions run <package-id> <analysis-id> --date YYYY-MM-DD
```

Component-only packages can be enabled without writing a connector config entry.
`doctor` starts each package server, checks health, and verifies its manifest
endpoint.

The menubar app has an Extensions view backed by the same CLI JSON contract. It
lists installed packages and read-only core component packages, shows their
capture/processor/analysis/connector components, toggles external package
enablement, runs `doctor`, and opens the global extension folder.

Built-in components are exposed as virtual packages, not installed packages:

| Virtual package | Captures | Processor surface |
| --- | --- | --- |
| `alvum.audio` | `alvum.audio/audio-mic`, `alvum.audio/audio-system` | `alvum.audio/whisper` |
| `alvum.screen` | `alvum.screen/snapshot` | `alvum.screen/vision` |
| `alvum.session` | `alvum.session/claude-code`, `alvum.session/codex` | `alvum.session/*-parser` |

They appear in `alvum extensions list --json` under `core` with
`read_only: true`. Their capture lifecycle still belongs to the signed
Electron/Rust path so macOS TCC remains stable.

The fastest developer loop is:

```bash
alvum extensions scaffold ./scratch-extension --id scratch --name "Scratch"
alvum extensions install ./scratch-extension
alvum extensions enable scratch
alvum extensions doctor
```

External connectors are configured in `~/.alvum/runtime/config.toml`:

```toml
[connectors.github]
enabled = true
kind = "external-http"
package = "github"
connector = "activity"
```

## Providers

The current provider implementations are Claude CLI, Codex CLI, Anthropic API,
Bedrock, Ollama, and `auto` fallback.

```bash
alvum providers list
alvum providers test --provider codex-cli
alvum providers models --provider ollama
alvum providers install-model --provider ollama --model gemma4:e2b
printf '{"settings":{"base_url":"http://localhost:11434","model":"llama3.2"}}' \
  | alvum providers configure ollama
alvum providers set-active codex-cli
alvum providers disable claude-cli
alvum providers enable claude-cli
```

Provider routing is still global for the core pipeline. `auto` only considers
providers enabled under `[providers.<id>]`; disabling a provider removes it from
Alvum's fallback list without uninstalling the CLI or deleting credentials.
The menu-bar Providers pane mirrors this: configured providers are listed on the
main page, Add Provider shows every known provider, and provider detail can use,
check, set up, configure, or remove a provider.

Provider setup stores non-secret fields in `~/.alvum/runtime/config.toml` and
stores provider secrets in macOS Keychain. Today this covers Anthropic API keys,
Ollama server/model settings, Bedrock profile/region/model settings, and optional
model overrides for Claude CLI and Codex CLI. `ANTHROPIC_API_KEY` and the
standard AWS credential chain remain supported as fallback paths.
Provider model dropdowns are best-effort: Ollama queries `/api/tags` and falls
back to parsing `ollama ls`, Anthropic queries `/v1/models`, Bedrock shells
through `aws bedrock list-foundation-models`, and Codex uses `codex debug
models`; providers fall back to safe defaults when a catalog cannot be reached.
Ollama also exposes curated download suggestions in the menu bar. Clicking
Download runs `ollama pull <model>` through Alvum, refreshes the installed-model
dropdown, and leaves the user's configured model unchanged until they explicitly
save a different model.
The Ollama detail pane also shows installed local models separately. `ollama
serve` is exposed as a setup action; if Terminal reports
`bind: address already in use`, the local Ollama server is already running and
Alvum should be able to query `http://localhost:11434/api/tags`.

Analysis extensions use the Alvum LLM broker, which calls the configured
provider and emits normal LLM events.

## Documentation Map

- `AGENTS.md` - operational guidance for rebuilds, signing, TCC, capture, and
  briefing debugging.
- `docs/superpowers/specs/2026-04-03-alvum-system-design.md` - product vision
  and long-term architecture.
- `docs/superpowers/specs/2026-04-18-storage-layout.md` - authoritative
  `~/.alvum/` layout.
- `docs/superpowers/specs/2026-04-28-platform-provider-briefing-gap-audit.md` -
  audit of platform/provider/briefing gaps.
- `docs/superpowers/specs/2026-04-28-component-extension-runtime.md` - external
  component extension runtime.
