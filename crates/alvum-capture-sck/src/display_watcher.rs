//! CoreGraphics display-reconfiguration watcher. Runs on a dedicated
//! OS thread with its own CFRunLoop so macOS can dispatch our callback.
//! On any significant change (sleep/wake, monitor hotplug, mode change),
//! we spawn a detached worker that calls `super::restart()` — the
//! callback itself must return quickly, so teardown/rebuild happens
//! off-thread.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

type CGDirectDisplayID = u32;
type CGDisplayChangeSummaryFlags = u32;
type CGError = i32;
type Callback = unsafe extern "C" fn(CGDirectDisplayID, CGDisplayChangeSummaryFlags, *mut c_void);

// Flags documented in <CoreGraphics/CGDirectDisplay.h>.
const FLAG_BEGIN_CONFIG: u32 = 1 << 0;
const FLAG_MOVED: u32 = 1 << 1;
const FLAG_SET_MAIN: u32 = 1 << 2;
const FLAG_SET_MODE: u32 = 1 << 3;
const FLAG_ADD: u32 = 1 << 4;
const FLAG_REMOVE: u32 = 1 << 5;
const FLAG_ENABLED: u32 = 1 << 8;
const FLAG_DISABLED: u32 = 1 << 9;
const FLAG_MIRROR: u32 = 1 << 10;
const FLAG_UN_MIRROR: u32 = 1 << 11;
const FLAG_DESKTOP_SHAPE: u32 = 1 << 12;
const SIGNIFICANT: u32 = FLAG_MOVED
    | FLAG_SET_MAIN
    | FLAG_SET_MODE
    | FLAG_ADD
    | FLAG_REMOVE
    | FLAG_ENABLED
    | FLAG_DISABLED
    | FLAG_MIRROR
    | FLAG_UN_MIRROR
    | FLAG_DESKTOP_SHAPE;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayRegisterReconfigurationCallback(
        callback: Callback,
        user_info: *mut c_void,
    ) -> CGError;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRun();
}

static STARTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_reconfig(
    _display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    _user_info: *mut c_void,
) {
    // The "begin" event fires *before* the reconfiguration — SCK can't
    // rebuild against a mid-transition display graph, so skip it and
    // wait for the post-change event.
    if flags & FLAG_BEGIN_CONFIG != 0 {
        return;
    }
    if flags & SIGNIFICANT == 0 {
        return;
    }
    info!(
        flags = format!("{:#x}", flags),
        "display reconfiguration detected; restarting SCK"
    );
    // Do the heavy lifting on a scratch thread so we return from the
    // CG callback immediately (macOS stalls further events otherwise).
    std::thread::spawn(|| {
        if let Err(e) = super::restart() {
            warn!(error = %e, "SCK restart after display change failed");
        }
    });
}

/// Start the watcher thread once per process. No-op on subsequent
/// calls. Safe to call from anywhere.
pub fn start_once() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("alvum-sck-display-watcher".into())
        .spawn(|| {
            let status = unsafe {
                CGDisplayRegisterReconfigurationCallback(on_reconfig, std::ptr::null_mut())
            };
            if status != 0 {
                warn!(
                    status,
                    "CGDisplayRegisterReconfigurationCallback failed; no proactive SCK recovery"
                );
                return;
            }
            info!("SCK display watcher armed");
            unsafe { CFRunLoopRun() };
        })
        .expect("spawn display watcher thread");
}
