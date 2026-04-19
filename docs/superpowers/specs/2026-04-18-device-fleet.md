# Device Fleet

First-class device management for alvum: a typed model of every machine that captures signal or runs the pipeline, how those devices are discovered and paired, what each may collect, and how the system degrades gracefully when one goes offline.

This spec backs the `/devices` and `/connectors` routes, and is the substrate the wearable, iOS companion, Watch, and vehicle-bridge connectors plug into. It does NOT redesign the Connector/CaptureSource/Processor traits — those stay — but it adds the "which physical thing is this running on?" axis the existing traits are missing.

## Problem

The top-level spec names "the wearable" (ESP32-S3 clip-on) and "the box" (Mac appliance) and assumes the rest is implicit. The mockup (`~/git/alvum-frontend/devices.jsx`) proves that assumption is wrong: a realistic alvum deployment has **seven or more devices** participating at once — appliance, primary-driver Mac, ESP32 wearable, iPhone, secondary Mac, Apple Watch, CarPlay bridge — each with different capture capabilities, permission surfaces, power envelopes, and trust levels.

Without a device model:
- The user has no way to see or control what's collecting what, across a fleet that grows over time.
- The pipeline can't reason about evidence quality ("did this come from the wearable's mic or the distant room mic on the Studio?").
- Permission audits are impossible — macOS permissions live per-app-per-device and the user has no consolidated view.
- Pause/resume/forget semantics are undefined. What happens to in-flight captures when a device goes offline? What happens to historical captures when a device is removed?
- Discovery is undefined. How does a new MacBook find The Box?

The device fleet is an explicit first-class primitive. Every observation the pipeline ingests is attributable to exactly one device. The `/devices` page is a single-screen audit of what's running, what it's collecting, and what the OS has granted it.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│ The Box (Mac appliance)                                             │
│   • alvum-core, alvum-pipeline, alvum-web, models                   │
│   • mDNS: _alvum._tcp.local on port 3741                            │
│   • HTTP ingest endpoints for every peer device                     │
│   • Device registry: ~/.alvum/   │
│     devices/registry.json  ← authoritative list                     │
└──────────────────────────────▲──────────────────────────────────────┘
                               │ local-network HTTP + mDNS discovery
     ┌─────────────────────────┼─────────────────────────┐
     │                         │                         │
┌────┴─────────┐       ┌───────┴───────┐       ┌─────────┴─────────┐
│ Desktop      │       │ Mobile /      │       │ Wearable /        │
│ app peers    │       │ Watch peers   │       │ Vehicle peers     │
│ MacBook,     │       │ iPhone,       │       │ ESP32 pin,        │
│ Studio       │       │ Apple Watch   │       │ CarPlay bridge    │
└──────────────┘       └───────────────┘       └───────────────────┘
```

Every peer device runs an alvum client that:
1. Discovers The Box via mDNS (`_alvum._tcp.local`).
2. Pairs once (user confirms on The Box's `/devices` page — the new device shows as `paired` waiting for approval).
3. Announces its capabilities (`DeviceCapabilities`) and requested signals.
4. Uploads captured bytes over HTTPS (self-signed cert pinned at pairing) to connector-specific endpoints (e.g., `POST /api/ingest/audio`, `POST /api/ingest/frame`).
5. Heartbeats (`POST /api/device/heartbeat` every 60s) reporting battery, storage, sync backlog, firmware, location hint.

The Box is the single source of truth for device state. Peers are stateless in the sense that if wiped, re-pairing restores them; they do not hold long-term data beyond a local ring buffer for offline capture.

### Trust boundary

Pairing is the only moment a device escapes the default-deny posture. At pairing, the user:
- Sees the device's self-reported identity (name, model, kind).
- Chooses which signals this device may produce (`DeviceSignals`).
- Receives an out-of-band pairing code shown on The Box to confirm no MITM.
- Is issued a device-specific bearer token used for all subsequent API calls.

Tokens are revocable per device from `/devices`. Revoking a token leaves historical data intact (it's already ingested) but prevents any future ingestion or heartbeat.

## Data Model

### Device

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One physical or logical device participating in the fleet. Stored in
/// devices/registry.json on The Box; mirrored to peers at pairing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    /// Short stable identifier ("mbp", "pin", "watch"). Unique per fleet.
    pub id: String,
    /// User-facing name. Defaults from the OS at pairing; editable.
    pub name: String,
    pub kind: DeviceKind,
    /// Free-form role description ("Daily-driver capture", "Commute audio + location").
    pub role: String,
    pub status: DeviceStatus,
    pub model: String,
    /// Optional human-readable location hint ("Home · office", "On you").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_hint: Option<String>,
    /// Local network address of the device relative to The Box, if reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    pub firmware: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub power: PowerSource,
    /// Battery level [0.0, 1.0]. None when wired or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageStats>,
    /// Bytes buffered locally waiting to upload. Shown to the user as pressure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_pending_bytes: Option<u64>,
    /// Seconds since boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_s: Option<u64>,
    /// Signals the user has authorized this device to produce.
    pub collect: DeviceSignals,
    /// OS-level permission state for each signal this device could produce.
    /// Separate from `collect` — the user may enable a signal the OS hasn't granted yet.
    pub permissions: DevicePermissions,
    /// Master enable. Pausing stops ingestion and marks Evidence confidence Low.
    pub enabled: bool,
    /// Free-form note ("Update available · 1.4.2").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// The Box. Hosts pipeline, UI, models.
    Appliance,
    /// A Mac running alvum-app in capture-peer mode (laptop, Studio).
    DesktopApp,
    /// ESP32-S3 clip-on wearable.
    Wearable,
    /// iPhone running the alvum iOS companion.
    Phone,
    /// Apple Watch running the alvum watchOS companion.
    Watch,
    /// CarPlay Bridge (dedicated hardware for commute capture).
    VehicleBridge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    /// Reachable and actively heartbeating.
    Online,
    /// Reachable but quiet (laptop lid closed, phone in pocket).
    Idle,
    /// Seen during pairing, awaiting user approval on The Box.
    Paired,
    /// Not heard from in > 5 min.
    Offline,
    /// User explicitly paused. Does not count toward signal aggregation.
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    Wired,
    Battery,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct StorageStats {
    pub used_gb: f32,
    pub total_gb: f32,
}
```

### DeviceSignals

Every signal a device could produce. User-controlled per device, bounded by `DeviceCapabilities` (below) which is device-reported.

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSignals {
    #[serde(default)] pub audio: bool,
    #[serde(default)] pub screen: bool,
    #[serde(default)] pub location: bool,
    #[serde(default)] pub camera: bool,
    #[serde(default)] pub motion: bool,
    #[serde(default)] pub imu: bool,
    #[serde(default)] pub heart_rate: bool,
    #[serde(default)] pub sleep: bool,
    #[serde(default)] pub workouts: bool,
    #[serde(default)] pub clipboard: bool,
    /// Typing cadence only. Content is NEVER captured. Enforced by the peer client.
    #[serde(default)] pub keystrokes: bool,
    #[serde(default)] pub photos: bool,
}
```

### DeviceCapabilities

What the device itself says it *can* produce. Device-reported at pairing and at every firmware upgrade. Serves as the upper bound of `DeviceSignals` — the UI never lets the user enable a signal the device doesn't advertise.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceCapabilities {
    pub signals: DeviceSignals,
    /// Max bytes the device can buffer offline before dropping oldest.
    pub local_buffer_bytes: u64,
    /// Codec hints for negotiation at the connector level.
    #[serde(default)]
    pub audio_codecs: Vec<String>,    // "opus", "aac"
    #[serde(default)]
    pub image_codecs: Vec<String>,    // "webp", "jpeg"
}
```

### DevicePermissions

OS-level permission state. Distinct from `DeviceSignals`: a user may want audio but the OS hasn't granted mic access yet. The UI surfaces both.

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevicePermissions {
    #[serde(default)] pub microphone: PermissionState,
    #[serde(default)] pub screen_recording: PermissionState,
    #[serde(default)] pub accessibility: PermissionState,
    #[serde(default)] pub location: PermissionState,
    #[serde(default)] pub camera: PermissionState,
    #[serde(default)] pub motion: PermissionState,
    #[serde(default)] pub photos: PermissionState,
    #[serde(default)] pub health: PermissionState,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    #[default]
    NotRequested,
    Granted,
    Denied,
    /// iOS-specific: location granted but only while the app is active.
    WhileInUse,
    /// iOS-specific: location granted always.
    Always,
}
```

### Heartbeat

Sent by every peer device at most once per 60s, more frequently when battery/sync state changes materially.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub device_id: String,
    pub ts: DateTime<Utc>,
    pub battery: Option<f32>,
    pub power: PowerSource,
    pub storage: Option<StorageStats>,
    pub sync_pending_bytes: Option<u64>,
    pub uptime_s: u64,
    pub location_hint: Option<String>,
    /// Permission state the peer observes locally. The Box reconciles with its
    /// stored state and surfaces drift on /devices.
    pub permissions: DevicePermissions,
}
```

### Signal Attribution on Observations

Every `Observation` the pipeline ingests gains two new fields so evidence can be traced back to the device:

```rust
// in alvum-core::observation (addition)
pub device_id: String,                    // "pin", "mbp", "iphone"
pub device_kind: DeviceKind,              // denormalized for filtering without a join
```

This is how the mockup can show "Mic · System · Location" badges on timeline blocks and why confidence scoring can weight "wearable near your mouth" higher than "distant room mic on the Studio."

## Discovery and Pairing

### mDNS advertisement (from The Box)

```
Service: _alvum._tcp.local
Port:    3741
TXT:
  app=alvum
  version=1.4.2
  fleet_id=<opaque 16-byte hex>  # so peers don't confuse two Boxes on a shared network
```

### Pairing flow

1. Peer scans for `_alvum._tcp.local` on its current subnet.
2. Peer POSTs `/api/device/pair` with its `DeviceCapabilities`, proposed `DeviceKind`, `name`, `model`, `firmware`, and an ephemeral X25519 public key.
3. The Box responds with its own ephemeral public key and a 6-digit pairing code displayed in the `/devices` UI under a "Pairing requests" section.
4. User types the code on the peer to confirm; peer completes the exchange and is issued a bearer token.
5. The Box writes the new Device to `devices/registry.json` with `status: Paired`. The user then enables the signals the peer is allowed to send, flipping it to `Online` on the next heartbeat.

If no user action on The Box within 5 minutes, the request is dropped. Pairing codes are never transmitted; the user types them manually.

### Certificate pinning

The Box generates a self-signed cert at first run. Its fingerprint is shown during pairing; the peer pins it. Subsequent ingest calls must present the matching cert. If the fingerprint changes (reinstalled Box), every peer must re-pair — this is by design.

## Storage

```
~/.alvum/runtime/
└── devices/
    ├── registry.json          ← authoritative Vec<Device>
    ├── tokens.json            ← bearer tokens per device (NEVER in git, 0600)
    └── heartbeats/
        └── <device_id>.jsonl  ← append-only Heartbeat log (14-day retention)
```

Under `runtime/` because the device registry is operational state — regenerable by re-pairing and never backed up (tokens should not leave the machine). Heartbeats are ephemeral debug logs.

`registry.json` is read on startup by The Box and whenever `/devices` is rendered; written under an advisory lock on every mutation (pair, pause, forget, permission state change).

Heartbeat logs inform the 14-day sparkline the `/devices` row shows for "last seen" and "battery"; they are pruned by the same retention daemon that handles raw capture.

## Pipeline Integration

1. **Ingest routing.** Each `POST /api/ingest/<type>` call carries `device_id` from the bearer token. The receiving handler writes a `DataRef` with `device_id` in the metadata, then dispatches to the matching `Processor`. No processor sees a DataRef missing a `device_id`.

2. **Evidence confidence floor.** An observation produced by a `Paused` device (retroactively pausing after capture) keeps its data but drops to `Evidence::Confidence::Low`. A `Forgotten` device's data is deleted from `capture/` but Decision objects that cite it keep the evidence chain with a source that resolves to "[device removed]".

3. **Alignment gap reporting.** The morning briefing (§ Pipeline — Brief) notes when a device the user depends on has been offline for > 12 hours: *"The wearable hasn't synced since yesterday; today's off-desk signal is weaker than normal."* This is driven by Device.status and last heartbeat.

4. **Budgeting.** Per-device sync caps in `AlvumConfig` prevent a runaway wearable from filling The Box:
   ```toml
   [devices.limits]
   per_device_daily_gb = 8
   wearable_daily_gb = 2     # override for Wearable kind
   ```

## /devices Page Contract

The web UI at `/devices` renders:
- Header: `N of M online · S signals feeding the pipeline`, "Add device" button.
- A 12-signal aggregation strip with count per signal across enabled devices.
- Filter tabs: All / Online / Idle / Offline.
- Per-device rows (collapsed): icon, name, kind, status dot, last-seen, battery, signals active count, master toggle. Expanding reveals:
  - Collection toggles with short privacy copy ("Mic feed — transcribed locally, raw audio discarded after 24h").
  - Device meta (model, role, network IP, firmware, uptime, local buffer).
  - "OS permissions" chip row showing each `PermissionState`.
  - Actions: Sync now, Rename, Forget.

Action semantics:
- **Master toggle → off**: `enabled = false`, status becomes `Paused`, ingest rejects new bytes from this device until re-enabled.
- **Forget**: deletes the device from registry, revokes its token, and removes `capture/*/by-device/<device_id>/` for the next retention pass. Historical Decision evidence retains a sentinel reference.
- **Sync now**: sends a command via long-poll / push channel asking the device to flush its local buffer.

The "Everything stays on The Box" footer ships in the page; do not remove it without a spec update — it's a user-facing privacy promise.

## Connector Relationship

A `Connector` (existing trait) is a *capability*; a `Device` is a *location where that capability runs*. One connector can be realized by multiple devices:

- The `audio` connector runs on: the appliance itself (if user chose), the MacBook (daily driver), the wearable pin, the CarPlay bridge.
- The `screen` connector runs on: the MacBook and Studio only.
- The `location` connector runs on: the iPhone (primary), the MacBook (when at desk, via CoreLocation), the wearable pin (coarse GPS when outdoors).

Connectors advertise which `DeviceKind`s they support via a new method on the trait:

```rust
pub trait Connector: Send + Sync {
    fn name(&self) -> &str;
    /// Which DeviceKinds can realize this connector.
    fn supported_device_kinds(&self) -> &[DeviceKind];
    // existing: capture_sources, processors, from_config
}
```

The `/connectors` page then shows a connector × device matrix, not just a flat enable/disable list.

## Privacy Guarantees (user-facing)

These are commitments the device fleet honors; violating them is a spec bug:

1. **Keystrokes capture is cadence-only.** The peer never serializes the character or keycode. Only interval statistics (inter-key milliseconds, bursts). Enforced at the peer; The Box rejects payloads containing `content` for the `keystrokes` signal.
2. **Clipboard text is redacted.** The peer strips anything matching a known-secret pattern (API keys, credit-card numbers, private keys) and tags the payload with what was redacted.
3. **Wearable camera frames blur non-consenting faces.** Face detection runs on-device; non-starred faces are blurred before upload. The Box can't undo this — the unblurred frame never leaves the pin.
4. **Device pause is hard-stop.** A paused device's peer client stops capture entirely; it does not cache locally pending re-enable. (Rationale: users pause because they don't want something recorded; caching until re-enable would defeat the point.)
5. **Forget is destructive.** Raw bytes referencing only this device are deleted within 24 hours. Derived data (transcripts, decisions) retains a `[device removed]` sentinel to keep the graph intact.

## Open Questions

- **Second-Box scenarios.** Alvum on two machines (home and office) — do they federate, mirror, or stay independent? Spec leans independent for V1 (you pick your Box); multi-Box is V3.5 via Syncthing per top-level growth path. No device belongs to two Boxes.
- **Device conflict detection.** Two devices both with `audio: true` in the same room produce duplicate audio streams. Prepare stage (§ Pipeline — Prepare) dedups by SNR, but the UI should show "these two devices will produce overlapping signal" at pairing time. Defer to Phase D capture-completeness plan.
- **Non-macOS peers.** The Windows or Linux MacBook-equivalent isn't V1 but the data model should not preclude it. `DeviceKind::DesktopApp` is deliberately OS-agnostic.
- **Guest mode.** Short-lived devices (visiting laptop) that auto-expire. Nice to have, not V1.

## Phase / Milestone

This spec is implemented across Phase C (product surface), Phase D (capture completeness), and Phase E (wearable):

| Component | Phase |
|---|---|
| `Device`, `DeviceSignals`, `DevicePermissions`, `Heartbeat` types in `alvum-core` | C |
| `devices/registry.json` storage helpers, `AlvumPaths::devices_*` methods | C |
| `POST /api/device/pair` and heartbeat endpoints in `alvum-web` | C |
| `/devices` UI page | C |
| Attribution fields on `Observation` | C (with alignment primitives) |
| Connector `supported_device_kinds` method | C |
| mDNS advertisement on macOS | C (part of Electron shell work) |
| ESP32 pairing client firmware | E |
| iOS companion pairing client | E (after wearable) |
| watchOS companion pairing client | E |

## Relationship to Other Specs

- **Top-level spec § Capture Layer:** extends that section. The capture layer's three "audio streams" (mic, system, wearable) become N streams, each attributable to a Device.
- **Top-level spec § Wearable:** remains accurate for the ESP32 specifics, but pairing and ingest flow through the endpoints this spec defines.
- **`2026-04-18-location-map.md`:** the iPhone and MacBook both produce location data; this spec tells the pipeline which device each sample came from so fusion can weight them.
- **`2026-04-18-health-connector.md`:** the Watch is a device with `heart_rate`, `sleep`, `workouts` capabilities. That spec's health samples ride these pairing + ingest rails.
