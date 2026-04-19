# Location and Map

Multi-source geospatial understanding for alvum: how raw GPS from multiple devices and third-party sources fuses into point / route / transit observations, how those observations are stored, how they're rendered on the timeline map, and what privacy invariants the system guarantees.

This spec supersedes the sparse "§ Capture — Location (CoreLocation)" bullet in the top-level spec (which assumed a flat `location.jsonl` of coordinates) and backs the location-aware components in `/timeline/:date`, `/decisions` (places referenced), and `/knowledge` (place entities).

## Problem

Location in the top-level spec is one line: *"CoreLocation significant-change monitoring (low power). Appended to `location.jsonl`."* That was enough for V0 but misses what a real day actually looks like (see `~/git/alvum-frontend/data.jsx:92-134`):

- **A single commute involves multiple modalities** — walk to subway, two trains with a transfer, walk to office. Flat GPS points can't describe this; a step-by-step transit structure is required.
- **Location data has multiple sources** — iPhone CoreLocation (continuous baseline), Strava (workouts with accurate pace/route), system-wide macOS CoreLocation (when at the desk), the wearable pin (coarse outdoor GPS), CarPlay bridge (drive routes). Each has different fidelity, battery cost, and privacy profile; the pipeline needs to choose the best source per block.
- **The UI renders a stylized map, not a slippy map** — normalized coordinates and a paper-map aesthetic (`~/git/alvum-frontend/map.jsx`). The underlying data must be real-world (lat/lon) but the rendering layer wants semantic hooks ("Park Slope", "Atlantic Av station") that raw points don't carry.
- **Location is privacy-sensitive** — home and certain hub locations should be redactable without breaking the timeline. Current spec has no notion of a "home zone" or lat/lon suppression.
- **Location is load-bearing evidence** — "stayed at the office" is the entire reason a gym intention got flagged as violated. Current spec can't feed this into the alignment engine because it has no typed location semantics.

This spec defines a typed `LocationObservation`, a fusion policy for overlapping sources, a storage layout that separates raw GPS from derived semantic events, and the UI rendering contract for the map component.

## Architecture

```
Raw per-device location streams
────────────────────────────────────────────────────────────────
  ╔══ iPhone CoreLocation ══╗      continuous, coarse (2-min cadence)
  ╔══ macOS CoreLocation ══╗       sporadic, precise (desk / office)
  ╔══ Wearable pin GPS ════╗       when outdoors, low fidelity
  ╔══ CarPlay Bridge ══════╗       continuous during drives
                 │
                 ▼
        per-device SNR / accuracy tags
                 │
                 ▼
Third-party sources (pull)
────────────────────────────────────────────────────────────────
  Strava workouts   HealthKit workouts   Photo EXIF   Manual entry
                 │
                 ▼
┌────────────────────────────────────────────────────────────────┐
│ Prepare stage — Location Fusion (§ Pipeline — Prepare)         │
│   1. Per-minute union across sources.                          │
│   2. Snap point clouds to routes when motion-consistent.       │
│   3. Detect transit via MapKit-style step resolution (walk,    │
│      subway, bus, drive, bike) with external APIs.             │
│   4. Emit LocationObservation stream with attributed source.   │
└────────────────────────────────────────────────────────────────┘
                 │
                 ▼
  Pipeline — Align stage (as one modality among many)
  Timeline UI (/timeline/:date) — rendered on the day map
  Entities (/knowledge) — place entities accumulated over time
  Alignment evidence — "stayed at office", "didn't go to gym"
```

Location is not a standalone thing to look at — it's a first-class evidence modality the alignment engine treats alongside audio and screen. The map is a *view* over observations already in the decision graph; there's no separate location index.

## Data Model

### LocationObservation

The core unit. One LocationObservation describes a continuous placement in the world — a point where you were, a route you took, or a multi-step trip. Multiple can overlap in time (a phone in your pocket on a walk + the wearable's GPS on the same walk → two observations from different sources that the fusion stage reconciles).

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocationObservation {
    /// Start timestamp.
    pub start: DateTime<Utc>,
    /// End timestamp. For Point kinds this is typically start + dwell duration.
    pub end: DateTime<Utc>,
    pub kind: LocationKind,
    pub source: LocationSource,
    /// The device id that produced the raw data (if from a device; None for pulled sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Human-readable label if known ("Prospect Park loop", "Home · Park Slope", "Walk to Mamoun's").
    /// Labels come from reverse geocoding, place-entity matching, or transit resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Free-form detail string for UI ("5.2 mi · 8'42\" pace · 312 ft gain").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Confidence in the observation overall (accuracy, source trust).
    pub confidence: LocationConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocationKind {
    /// Stationary at a location for a meaningful duration (> 2 min default).
    Point {
        at: GeoPoint,
        /// Reverse-geocoded place id (from a known PlaceEntity) if matched.
        place_id: Option<String>,
    },
    /// Continuous motion over a traced path (run, walk, bike, drive without transit handoff).
    Route {
        points: Vec<GeoPoint>,
        mode: MotionMode,
        /// Meters over the route.
        distance_m: Option<f32>,
        /// Elevation gain in meters.
        gain_m: Option<f32>,
        /// Moving seconds (excludes stopped time). Useful for pace.
        moving_s: Option<u32>,
    },
    /// Multi-modal trip with explicit steps (typical commute).
    Transit {
        origin: GeoPoint,
        destination: GeoPoint,
        steps: Vec<TransitStep>,
        /// Door-to-door duration including transfers.
        total_duration_s: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    /// When this point was sampled. For Route points, this is the per-sample timestamp;
    /// for Point.at, this is the dwell start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
    /// Horizontal accuracy in meters, as reported by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MotionMode {
    Walk,
    Run,
    Bike,
    Drive,
    /// Motorcycle / scooter / etc. — lumped for V1.
    Motor,
    /// Unknown; source didn't classify.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitStep {
    pub kind: TransitStepKind,
    /// Human-readable ("Walk to 7 Av station", "B train → Atlantic Av–Barclays").
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f32>,
    /// Transit line identifier if applicable ("B", "Q", "F·G", "M62").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Number of stops for subway/bus/train steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stops: Option<u32>,
    /// Optional traced path for this leg (for rendering).
    #[serde(default)]
    pub points: Vec<GeoPoint>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitStepKind {
    Walk,
    Subway,
    Bus,
    Train,
    Drive,
    Bike,
    /// Zero-distance handoff between two transit modes.
    Transfer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationSource {
    /// From the iPhone companion (CoreLocation).
    Iphone,
    /// From the Mac running macOS CoreLocation.
    Mac,
    /// From the wearable pin GPS.
    Wearable,
    /// From CarPlay bridge.
    CarPlay,
    /// Pulled from Strava API.
    Strava,
    /// Pulled from Apple HealthKit workout routes.
    HealthKit,
    /// Extracted from photo EXIF.
    PhotoExif,
    /// Manually entered or corrected by the user.
    Manual,
    /// Inferred by the pipeline without a raw GPS source (e.g., office because calendar event).
    Inferred,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationConfidence {
    /// Multi-source convergence OR direct device GPS with < 30m accuracy.
    High,
    /// Single source with reasonable accuracy, OR inferred from calendar.
    Medium,
    /// Weak signal, low accuracy (> 100m), or inference with no corroboration.
    Low,
}
```

### PlaceEntity

Meaningful places the user visits repeatedly become `PlaceEntity` — the location equivalent of a Knowledge Corpus `Entity`. Stored in `knowledge/entities.jsonl` with `entity_type: "place"` per existing conventions; this spec defines the structured attributes a place entity carries.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaceAttributes {
    /// Canonical point at the place (centroid if polygon).
    pub at: GeoPoint,
    /// Radius in meters considered "the same place" for dwell detection.
    #[serde(default = "default_place_radius_m")]
    pub radius_m: f32,
    /// Optional category hint ("home", "office", "gym", "coffee_shop", "park").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Privacy class — controls whether this place's raw coordinates may leave The Box.
    pub privacy: PlacePrivacy,
}

fn default_place_radius_m() -> f32 { 50.0 }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlacePrivacy {
    /// Normal — raw coordinates usable anywhere.
    Public,
    /// Obfuscate in any UI export, map screenshot, or outbound sync.
    /// Home and any place the user tags as sensitive default here.
    Sensitive,
    /// Never render raw coordinates; always show label only ("Home").
    /// Even the on-disk LocationObservation is stored with the place_id
    /// and not the lat/lon (see "Storage" below).
    Hidden,
}
```

## Fusion Policy (Prepare stage)

The Prepare stage (§ Pipeline — Stage 1) runs location fusion as a deterministic step before any LLM touches the data. Rules applied in order:

1. **Per-minute union**: for each 60-second bucket, collect every raw point from every source. Skip buckets with no samples.

2. **Source ranking** for accuracy ties, most-trusted first:
   1. Strava (for workouts — we trust their route cleanup)
   2. HealthKit (for workouts without Strava)
   3. iPhone with `accuracy_m < 30`
   4. Wearable pin outdoors
   5. CarPlay (during drives only)
   6. Mac (for desk / known indoor locations)
   7. iPhone with `accuracy_m >= 30` (fallback)

3. **Dwell detection**: a cluster of points within 50 m of each other for > 2 min becomes a `Point`. The label is resolved via PlaceEntity matching first, then reverse geocoding (cached per hex-cell for 30 days).

4. **Route vs. Transit classification**: a continuous moving segment with a single `MotionMode` becomes a `Route`. A segment with mode transitions and speed profiles consistent with public transit (e.g., sharp speed drops at station coordinates, rapid acceleration matching subway curves) becomes a `Transit`, resolved through a transit API (MapKit on macOS, static GTFS schedules as fallback).

5. **Home suppression**: any raw point within a Hidden-privacy PlaceEntity's radius is stored as a Point referencing only the place_id, with lat/lon set to the place centroid. The underlying raw stream is discarded at ingest for Hidden places — this is the only case where location bytes are dropped before the 30-day retention window.

6. **Gap tolerance**: gaps < 3 min between samples of the same source are interpolated linearly; gaps >= 3 min break the observation into separate records.

## Storage

```
~/.alvum/
├── capture/YYYY-MM-DD/
│   └── location/
│       ├── raw/
│       │   ├── iphone.jsonl        ← raw CoreLocation samples
│       │   ├── wearable.jsonl
│       │   ├── mac.jsonl
│       │   └── carplay.jsonl
│       ├── pulled/
│       │   ├── strava.json         ← Strava activity manifests for this day
│       │   └── healthkit.json      ← HealthKit workout routes for this day
│       └── fused.jsonl             ← Vec<LocationObservation> after Prepare
└── generated/
    ├── knowledge/
    │   └── entities.jsonl          ← PlaceEntity entries (entity_type: "place")
    └── days/
        └── YYYY-MM-DD.json         ← DayExtraction references fused LocationObservations
                                      by index (not duplicated)
```

### Retention

- `capture/YYYY-MM-DD/location/**` — indefinite per the storage-layout spec default. Future re-extraction benefits from the full history (map-matching improves, transit APIs get richer, a new activity-classification model finds runs that the current one missed).
- `generated/days/` and `generated/knowledge/entities.jsonl` — forever. Derived, small, load-bearing.
- Places with `PlacePrivacy::Hidden` — raw samples within their radius are never written to `raw/*`. Only the fused Point record exists, with coordinates suppressed to the place centroid. This is destructive at ingest and does NOT benefit from re-extraction; by design.

### Hex-cell reverse geocode cache

To avoid re-hitting a geocoder on every restart (lives under `runtime/` since it's regenerable):

```
~/.alvum/runtime/cache/geocode/
└── h3-r9.jsonl                     ← H3 resolution-9 cell → label (30-day TTL)
```

One line per cell with label, category, last-resolved timestamp. Deleted cells are re-resolved on next visit; eviction is by age, not by access.

## Pipeline Integration

### As alignment evidence

`LocationObservation` entries for the day are loaded alongside other observations. The alignment engine sees them as structured `Observation.metadata` with a `location` key (see "Observation extension" below). Example evidence chain the engine can construct:

> Intention: "Run 3×/week" (Habit, cadence: 3/weekly)
>   Evidence found:
>     - No Route with MotionMode::Run between 06:00–08:00 today
>     - Point at Home until 20:05 today
>     - Point at Office 09:04–18:35 today
>   → AlignmentStatus::Drifting { gap_description: "No run today; stayed at office through 18:35." }

### As decision context

Decisions gain implicit location context from the timeline block they were extracted from. The `Decision.evidence` chain can cite a LocationObservation by its `(date, index_in_fused)` tuple. UI then links from the decision to the map pin.

### As emergent-state input

`EmergentState` detection can reference location patterns: *"5 consecutive days staying > 10 hours at the office"* is an input to a potential "overwork" state.

### Observation extension

`alvum-core::observation::Observation` already has a `metadata: Option<serde_json::Value>` field; location data rides there with a stable shape:

```json
"metadata": {
  "location": {
    "kind": "point",
    "label": "Office · Midtown",
    "place_id": "place_office_midtown"
  }
}
```

Or for a route:

```json
"metadata": {
  "location": {
    "kind": "route",
    "label": "Walk home via Gowanus",
    "mode": "walk",
    "distance_m": 3400,
    "source": "iphone"
  }
}
```

This keeps the Observation shape stable; location is just one structured payload in `metadata`.

## /timeline/:date Map Rendering Contract

The web UI map is stylized (see `~/git/alvum-frontend/map.jsx`) — paper aesthetic, normalized coordinates, hand-drawn neighborhoods and subway lines. Rendering must not require a slippy-map tile server. Contract for the component:

1. **Input**: `Vec<LocationObservation>` for a single day, a selected observation id, and a viewport (bounding box).
2. **Projection**: the UI picks a `ViewportProjection { center, zoom_bounds }` derived from the day's observations. Raw GPS is projected to SVG coordinates at render time; normalized `[0..1]` coordinates appear only in the SVG layer, never on disk.
3. **Layers** (bottom-up):
   1. Paper background + street-grid overlay.
   2. Named neighborhood labels.
   3. Parks, water, prominent landmarks (driven by Apple Maps geocoding at viewport resolve time, cached).
   4. Subway/bus lines for the day's Transit steps (drawn with agency line colors when `line` resolves to a known color; neutral otherwise).
   5. Day trail — the dashed polyline through every `Point.at` and each Route/Transit start/end, in chronological order.
   6. Route polylines for selected segments (Run, Walk, Drive) drawn with arrows at endpoints.
   7. Transit step decomposition for the selected Transit (stations marked, transfer icons at handoff points).
   8. Pins at each `Point` with dwell-duration badges; selected pin pulses.
4. **Privacy**: PlaceEntity `Hidden` places render as a labeled pin at their centroid with a neutral icon; raw coordinates are not projected at all. `Sensitive` places render normally in-app but are redacted on any shareable export.
5. **Multiplicity**: when multiple observations share a pin position (office visited morning and afternoon), render a single pin with a count badge; click expands a timeline stub.

The component does not fetch remote tiles, does not leak coordinates to third-party CDNs, and does not cache raw coordinates in browser storage beyond the current session.

## Privacy Invariants

1. **Hidden places never leave The Box with coordinates.** Fusion writes the place centroid, not the raw lat/lon, and the raw capture is discarded at ingest.
2. **Home-zone radius is configurable.** Default 150 m. User sets once in onboarding; editable in `/settings → Privacy → Places`.
3. **Exports redact Sensitive.** Any briefing, decision export, or screenshot generation must re-project through the PlaceEntity table, replacing Sensitive coordinates with labels.
4. **Third-party ingest is opt-in per source.** Strava and HealthKit integrations require explicit authorization flows in `/connectors`; revocation instantly stops pulls and optionally deletes prior pulled data.
5. **Device-level pause applies.** Pausing a device in `/devices` stops its location ingest immediately; location already captured stays, but fusion for subsequent buckets will not include that source.

## Connector Relationship

Three new connectors bundle the primitives this spec requires:

- **`alvum-connector-location-mac`** — wraps macOS CoreLocation for the MacBook / Studio / Mac mini appliance when Mac itself is a capture device. No new capture primitive; thin wrapper over existing CoreLocation library use.
- **`alvum-connector-location-iphone`** — paired iPhone companion sends CoreLocation to `/api/ingest/location` via the device-fleet pairing rails (`2026-04-18-device-fleet.md`).
- **`alvum-connector-strava`** (and `-healthkit`) — one-shot pull connectors on a schedule; OAuth for Strava, HealthKit authorization for the iPhone / Watch. Output is `LocationObservation` records directly (no raw stream).

The Wearable ESP32 gets location capability via a future firmware revision with a GPS module; deferred beyond V1.5.

## Open Questions

- **Transit API cost.** Live transit resolution (station lookups, line colors, agency data) ideally hits Apple MapKit on-device. If that's not usable from Rust, fall back to bundled GTFS schedules for the user's home city (downloaded once during onboarding). Need a spike to confirm the MapKit approach.
- **Indoor location.** Above-ground GPS is fine; indoor (basement office, subway stations deep enough) is spotty. Rely on calendar + WiFi SSID (if the user grants access) as a fallback. Defer; not a V1.5 requirement.
- **Route joining.** A run that loses GPS under a bridge currently splits into two Routes. Fusion should probably heal this; exact heuristic needs field data to tune. Log and leave as-is for V1.5.
- **Map tile alternative.** The stylized paper map is beautiful but specific to rendering scale. A high-zoom user wants to see actual streets; switching to a local-only slippy-map tile source (cached OSM) would be a /settings toggle. Defer.
- **Cross-timezone travel.** LocationObservation stores UTC timestamps, but UI time formatting must consider the device's local time zone at the time of observation. Track timezone per Point/step; add `tz: Option<String>` to GeoPoint if needed.

## Phase / Milestone

| Component | Phase |
|---|---|
| `LocationObservation`, `LocationKind`, `TransitStep`, `GeoPoint` types in `alvum-core` | A (alignment primitives — these are data types alignment depends on) |
| Fusion implementation in `alvum-pipeline::prepare` | B |
| `alvum-connector-location-mac` | D (capture completeness) |
| `alvum-connector-location-iphone` via iOS companion | E (parallel with wearable) |
| `alvum-connector-strava` + `alvum-connector-healthkit` | E or later |
| Place-entity extraction / radius UI | C |
| `/timeline/:date` map component | C |
| H3 hex-cell geocode cache | C |
| PlacePrivacy enforcement in fusion + export | C |

Phase A is getting an extra type pass — add `LocationObservation` and friends alongside the other alignment primitives. The Prepare-stage fusion logic comes later (Phase B), but the types need to exist first for `Observation.metadata.location` to be typeable.

## Relationship to Other Specs

- **Top-level spec § Capture — Location:** superseded by this spec.
- **`2026-04-18-device-fleet.md`:** every `LocationObservation.source` that's not `Inferred` or `Manual` traces back to a `Device`. The pairing and ingest flow for iPhone / CarPlay Bridge / wearable lives there.
- **`2026-04-18-health-connector.md`:** workouts with routes produce `LocationObservation.kind = Route` with `source = HealthKit`. The health connector is the HealthKit ingest owner; this spec owns the resulting location shape.
- **`2026-04-18-alignment-primitives.md` (Phase A plan):** extended to include LocationObservation types in `alvum-core`.
