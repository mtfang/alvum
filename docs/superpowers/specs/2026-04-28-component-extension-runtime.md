# Component Extension Runtime

**Date:** 2026-04-28
**Status:** Runtime v1 implemented in this repo

This spec documents Alvum's external extension runtime. It is inspired by Pi's
self-contained extension packages and package-manager flow, but Alvum does not
load third-party extension code into its Rust process. Extension packages run
as Alvum-managed localhost HTTP services.

## Component Model

An extension package can ship four component types:

| Component | Role |
| --- | --- |
| Capture | Produces `DataRef`s, either by importing source data or running as a daemon. |
| Processor | Consumes `DataRef`s and emits final `Observation`s. |
| Analysis lens | Consumes brokered Alvum context and emits custom artifacts or graph overlays. |
| Connector | User-facing bundle that composes captures, processors, analyses, and default routes. |

Capture and processor components are independently addressable by fully
qualified IDs:

```text
package/component
```

Connectors own the default routing matrix. Fanout is allowed: one capture output
can route to multiple processors. Processor chaining is intentionally out of
scope for v1.

Routes use global component IDs, so a connector package can compose capture,
processor, and analysis components from other installed packages. Referenced
external component packages must be installed and enabled; component-only
packages can be enabled without adding a connector entry to user config.

Core Alvum components are also exposed as read-only virtual packages. They use
the same manifest shape and route IDs, but they are not installed under
`~/.alvum/runtime/extensions/` and Alvum never launches them as HTTP services:

| Virtual package | Capture components | Processor components |
| --- | --- | --- |
| `alvum.audio` | `alvum.audio/audio-mic`, `alvum.audio/audio-system` | `alvum.audio/whisper` |
| `alvum.screen` | `alvum.screen/snapshot` | `alvum.screen/vision` |
| `alvum.session` | `alvum.session/claude-code`, `alvum.session/codex` | `alvum.session/claude-code-parser`, `alvum.session/codex-parser` |

External connectors may route built-in capture output to external processors by
referencing these capture component IDs. Built-in capture lifecycle remains
inside the signed app/CLI path because macOS TCC grants depend on that process
tree.

## Manifest

Every package has an `alvum.extension.json` at package root:

```json
{
  "schema_version": 1,
  "id": "github",
  "name": "GitHub",
  "version": "0.1.0",
  "server": {
    "start": ["node", "dist/server.js"],
    "health_path": "/v1/health",
    "startup_timeout_ms": 5000
  },
  "captures": [
    {
      "id": "events",
      "display_name": "GitHub events",
      "sources": [{ "id": "github", "display_name": "GitHub", "expected": false }],
      "schemas": ["github.event.v1"]
    }
  ],
  "processors": [
    {
      "id": "summarize",
      "display_name": "GitHub summarizer",
      "accepts": [{ "component": "github/events", "schema": "github.event.v1" }]
    }
  ],
  "analyses": [
    {
      "id": "weekly-review",
      "display_name": "Weekly review",
      "scopes": ["all"],
      "output": "artifact"
    }
  ],
  "connectors": [
    {
      "id": "activity",
      "display_name": "GitHub activity",
      "routes": [
        {
          "from": { "component": "github/events", "schema": "github.event.v1" },
          "to": ["github/summarize"]
        }
      ],
      "analyses": ["github/weekly-review"]
    }
  ],
  "permissions": [
    { "kind": "network", "description": "Connects to api.github.com" }
  ]
}
```

## HTTP Contract

Alvum starts the package server with:

```text
ALVUM_EXTENSION_PORT
ALVUM_EXTENSION_TOKEN
ALVUM_EXTENSION_ID
ALVUM_EXTENSION_DATA_DIR
ALVUM_HOST_URL
```

The service must bind to `127.0.0.1:$ALVUM_EXTENSION_PORT` and require:

```text
Authorization: Bearer $ALVUM_EXTENSION_TOKEN
```

Required endpoints:

| Endpoint | Purpose |
| --- | --- |
| `GET /v1/health` | Startup health check. |
| `GET /v1/manifest` | Returns the same manifest. |
| `POST /v1/gather` | Returns `data_refs`, direct `observations`, and warnings. |
| `POST /v1/process` | Converts routed DataRefs into Observations. |
| `POST /v1/capture/start` | Starts daemon capture for a capture component. |
| `POST /v1/capture/stop` | Stops daemon capture. |
| `POST /v1/analyze` | Runs an analysis lens. |

## Routing Data

`DataRef` now includes routing identity:

```json
{
  "ts": "2026-04-11T10:15:00Z",
  "source": "github",
  "producer": "github/events",
  "schema": "github.event.v1",
  "path": "github/events.jsonl",
  "mime": "application/x-jsonl"
}
```

Old DataRefs without `producer` or `schema` still deserialize. Built-in
processors continue to work through source matching. External processors use
route selectors over producer, source, MIME, and schema.

## Analysis Lenses

Analysis lenses are the extension version of a briefing or decision-graph view.
They do not mutate canonical `briefing.md`, `decisions.jsonl`, or
`tree/L4-edges.jsonl`.

During analysis runs, Alvum starts a local context/LLM broker. The extension
receives broker URLs in `/v1/analyze`:

```json
{
  "analysis": "weekly-review",
  "date": "2026-04-28",
  "output_dir": "/Users/michael/.alvum/generated/briefings/2026-04-28",
  "context_url": "http://127.0.0.1:49152/v1/context/query",
  "llm_url": "http://127.0.0.1:49152/v1/llm/complete",
  "token": "..."
}
```

Context scopes must be declared in the manifest. `all` grants observations,
decisions, edges, briefing, knowledge, and raw-file blob references through the
broker. Raw files are exposed as authenticated blob URLs instead of broad
filesystem access.

Analysis outputs are written under:

```text
~/.alvum/generated/briefings/YYYY-MM-DD/extensions/<analysis-id>/
```

## Package Commands

```bash
alvum extensions scaffold ./my-extension --id my_extension --name "My Extension"
alvum extensions install /path/to/package
alvum extensions install git:https://example.com/repo.git
alvum extensions install npm:package-name
alvum extensions update <package-id>
alvum extensions remove <package-id>
alvum extensions enable <package-id> [--connector <connector-id>]
alvum extensions disable <package-id>
alvum extensions list
alvum extensions list --json
alvum extensions doctor
alvum extensions doctor --json
alvum extensions run <package-id> <analysis-id> --date YYYY-MM-DD
```

Installs validate the manifest and leave packages disabled by default.
NPM installs use `--ignore-scripts` and preserve the installed dependency tree
under the package directory. `doctor` validates the manifest, starts the managed
service, checks health, and verifies `/v1/manifest`.

`scaffold` writes a minimal Node-based package with a manifest, HTTP service,
sample capture, processor, analysis lens, and connector route. It is meant as
the lowest-friction starting point for authors, not as a required SDK.

`list --json` and `doctor --json` are the stable frontend contract used by the
menubar app. `list --json` returns installed external packages under
`extensions` and read-only built-in packages under `core`. Human-readable CLI
output can change; the JSON shape should remain compatible.

## Menubar Integration

The Electron menubar exposes an Extensions view next to Capture, Briefing, and
Providers. It does not read package files directly. Instead it calls:

```text
alvum extensions list --json
alvum extensions enable <package-id>
alvum extensions disable <package-id>
alvum extensions doctor --json
```

The UI can list installed packages and core packages, inspect their components,
toggle external package enabled state, run health checks, and open
`~/.alvum/runtime/extensions/`. Core packages are read-only in the Extensions
view; users manage their enabled state through the existing Capture and
connector configuration controls. The CLI and registry remain the source of
truth, so a package enabled from the menu bar is the same package used by
capture and briefing runs.

## Current Limitations

- Route overrides are not user-configurable in v1.
- Processor chaining is not supported.
- External processors own their own model/API dependencies; only analysis
  lenses use the Alvum LLM broker.
- Managed HTTP service startup is per operation, not a persistent shared
  process pool.
- Project-local auto-discovery is intentionally not supported.
- Built-in processors are cataloged, but external connectors do not instantiate
  built-in processors through the HTTP extension adapter in v1.
