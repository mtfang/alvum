//! Two capture triggers: app focus change detection and idle timer.
//!
//! App focus is detected by polling the frontmost application name every 500ms
//! from a dedicated OS thread (using the same core-graphics window enumeration
//! as screenshot.rs). When the app name changes, an AppFocus trigger fires.
//!
//! An idle timer fires every 30 seconds of inactivity. It resets whenever an
//! app focus change occurs.

use anyhow::Result;
use core_foundation::array::CFArrayGetValueAtIndex;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerKind {
    /// User switched to a different application.
    AppFocus,
    /// User switched windows within the same application (e.g., different tab, project).
    WindowFocus,
    /// No focus change for 30 seconds — capture current state.
    Idle,
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerKind::AppFocus => "app_focus",
            TriggerKind::WindowFocus => "window_focus",
            TriggerKind::Idle => "idle",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TriggerEvent {
    pub kind: TriggerKind,
    pub ts: chrono::DateTime<chrono::Utc>,
}

const IDLE_INTERVAL: Duration = Duration::from_secs(30);

/// Start the trigger system. Returns a receiver that yields TriggerEvents.
///
/// Spawns a dedicated OS thread for focus polling and a tokio task for the idle
/// timer. Both shut down when the returned receiver is dropped.
pub fn start_triggers() -> Result<mpsc::Receiver<TriggerEvent>> {
    let (tx, rx) = mpsc::channel::<TriggerEvent>(64);
    let (reset_tx, mut reset_rx) = mpsc::channel::<()>(16);

    // App focus polling on a dedicated OS thread (blocks on sleep)
    let focus_tx = tx.clone();
    std::thread::spawn(move || {
        let (mut last_app, mut last_window) = get_frontmost_window_info();
        debug!(app = %last_app, window = %last_window, "initial frontmost window");
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let (current_app, current_window) = get_frontmost_window_info();
            if current_app != last_app {
                info!(from = %last_app, to = %current_app, "app focus changed");
                last_app = current_app;
                last_window = current_window;
                let event = TriggerEvent {
                    kind: TriggerKind::AppFocus,
                    ts: chrono::Utc::now(),
                };
                if focus_tx.blocking_send(event).is_err() {
                    break;
                }
                let _ = reset_tx.blocking_send(());
            } else if current_window != last_window {
                info!(app = %current_app, from = %last_window, to = %current_window, "window focus changed");
                last_window = current_window;
                let event = TriggerEvent {
                    kind: TriggerKind::WindowFocus,
                    ts: chrono::Utc::now(),
                };
                if focus_tx.blocking_send(event).is_err() {
                    break;
                }
                let _ = reset_tx.blocking_send(());
            }
        }
    });

    // Idle timer (resets on app focus change)
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(IDLE_INTERVAL) => {
                    debug!("idle timer fired");
                    let event = TriggerEvent {
                        kind: TriggerKind::Idle,
                        ts: chrono::Utc::now(),
                    };
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
                Some(()) = reset_rx.recv() => {
                    debug!("idle timer reset");
                    continue;
                }
            }
        }
    });

    Ok(rx)
}

/// Get the frontmost application name and window title via core-graphics.
///
/// Returns (app_name, window_title) of the first on-screen, layer-0 window
/// belonging to a regular application. Window title changes when switching
/// tabs, projects, or documents within the same app.
fn get_frontmost_window_info() -> (String, String) {
    let options =
        window::kCGWindowListOptionOnScreenOnly | window::kCGWindowListExcludeDesktopElements;

    let Some(window_list) = window::copy_window_info(options, window::kCGNullWindowID) else {
        return ("Unknown".to_string(), String::new());
    };

    let key_owner = CFString::new("kCGWindowOwnerName");
    let key_name = CFString::new("kCGWindowName");
    let key_layer = CFString::new("kCGWindowLayer");
    let key_onscreen = CFString::new("kCGWindowIsOnscreen");

    let count = window_list.len();
    for i in 0..count {
        let dict: CFDictionary<CFString, CFType> = unsafe {
            let ptr = CFArrayGetValueAtIndex(window_list.as_concrete_TypeRef(), i as _);
            TCFType::wrap_under_get_rule(ptr as _)
        };

        // Skip windows not at layer 0 (normal application windows)
        if let Some(layer_val) = dict.find(&key_layer) {
            let layer_ref: CFNumber =
                unsafe { TCFType::wrap_under_get_rule(layer_val.as_CFTypeRef() as _) };
            if let Some(layer) = layer_ref.to_i32() {
                if layer != 0 {
                    continue;
                }
            }
        }

        // Skip windows not on screen
        if let Some(onscreen_val) = dict.find(&key_onscreen) {
            let onscreen: CFBoolean =
                unsafe { TCFType::wrap_under_get_rule(onscreen_val.as_CFTypeRef() as _) };
            if onscreen == CFBoolean::false_value() {
                continue;
            }
        }

        // Must have an owner name
        let Some(owner_val) = dict.find(&key_owner) else {
            continue;
        };
        let owner: CFString =
            unsafe { TCFType::wrap_under_get_rule(owner_val.as_CFTypeRef() as _) };
        let app_name = owner.to_string();

        if app_name == "Window Server" || app_name.is_empty() {
            continue;
        }

        // Window title (may be empty if permission not granted for window names)
        let window_title = dict.find(&key_name)
            .map(|v| {
                let s: CFString = unsafe { TCFType::wrap_under_get_rule(v.as_CFTypeRef() as _) };
                s.to_string()
            })
            .unwrap_or_default();

        return (app_name, window_title);
    }

    ("Unknown".to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_kind_as_str() {
        assert_eq!(TriggerKind::AppFocus.as_str(), "app_focus");
        assert_eq!(TriggerKind::WindowFocus.as_str(), "window_focus");
        assert_eq!(TriggerKind::Idle.as_str(), "idle");
    }

    #[test]
    #[ignore] // Requires window server access; run manually
    fn get_frontmost_window_returns_nonempty() {
        let (app, title) = get_frontmost_window_info();
        assert!(!app.is_empty());
        assert_ne!(app, "Unknown");
        eprintln!("frontmost: app='{}' title='{}'", app, title);
    }
}
