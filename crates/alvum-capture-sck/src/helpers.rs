//! Async→sync bridges over the ScreenCaptureKit completion-handler API.
//!
//! SCK exposes its lifecycle (content discovery, filter swap, stream
//! start) as async ObjC methods that take a completion block. The shared
//! stream is built and driven from synchronous Rust, so each call gets a
//! Condvar-gated wrapper that waits up to [`SCK_WAIT_TIMEOUT`] for the
//! callback before giving up.

use anyhow::{anyhow, Result};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{SCContentFilter, SCShareableContent, SCStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const SCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn get_shareable_content_blocking() -> Result<Retained<SCShareableContent>> {
    type Slot = Arc<Mutex<Option<Result<Retained<SCShareableContent>, String>>>>;
    let slot: Slot = Arc::new(Mutex::new(None));
    let signal = Arc::new((Mutex::new(false), Condvar::new()));

    let slot_cb = slot.clone();
    let signal_cb = signal.clone();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if content.is_null() {
                let msg = if !error.is_null() {
                    unsafe { (*error).localizedDescription().to_string() }
                } else {
                    "unknown SCShareableContent error".to_string()
                };
                Err(msg)
            } else {
                match unsafe { Retained::retain(content) } {
                    Some(r) => Ok(r),
                    None => Err("failed to retain SCShareableContent".into()),
                }
            };
            *slot_cb.lock().unwrap() = Some(result);
            let (lock, cvar) = &*signal_cb;
            *lock.lock().unwrap() = true;
            cvar.notify_one();
        },
    );

    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };

    let (lock, cvar) = &*signal;
    let done = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(done, SCK_WAIT_TIMEOUT).unwrap();
    let done = guard;
    if !*done {
        return Err(anyhow!(
            "SCShareableContent did not return within {:?}",
            SCK_WAIT_TIMEOUT
        ));
    }
    drop(done);

    slot.lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow!("SCShareableContent callback produced no result"))?
        .map_err(|e| anyhow!("SCShareableContent: {}", e))
}

pub(crate) fn update_content_filter_blocking(
    stream: &SCStream,
    new_filter: &SCContentFilter,
) -> Result<()> {
    let done = Arc::new((Mutex::new(false), Condvar::new(), AtomicBool::new(true)));
    let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let done_cb = done.clone();
    let err_cb = err_slot.clone();
    let block = RcBlock::new(move |err: *mut NSError| {
        if !err.is_null() {
            let msg = unsafe { (*err).localizedDescription().to_string() };
            *err_cb.lock().unwrap() = Some(msg);
            done_cb.2.store(false, Ordering::SeqCst);
        }
        *done_cb.0.lock().unwrap() = true;
        done_cb.1.notify_one();
    });

    unsafe { stream.updateContentFilter_completionHandler(new_filter, Some(&block)) };

    let (lock, cvar, ok_flag) = &*done;
    let finished = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(finished, SCK_WAIT_TIMEOUT).unwrap();
    let finished = guard;
    if !*finished {
        return Err(anyhow!(
            "SCStream updateContentFilter did not complete within {:?}",
            SCK_WAIT_TIMEOUT
        ));
    }
    if !ok_flag.load(Ordering::SeqCst) {
        let msg = err_slot.lock().unwrap().take().unwrap_or_else(|| "unknown".into());
        return Err(anyhow!("SCStream updateContentFilter error: {}", msg));
    }
    Ok(())
}

pub(crate) fn start_capture_blocking(stream: &SCStream) -> Result<()> {
    let done = Arc::new((Mutex::new(false), Condvar::new(), AtomicBool::new(true)));
    let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let done_cb = done.clone();
    let err_cb = err_slot.clone();
    let block = RcBlock::new(move |err: *mut NSError| {
        if !err.is_null() {
            let msg = unsafe { (*err).localizedDescription().to_string() };
            *err_cb.lock().unwrap() = Some(msg);
            done_cb.2.store(false, Ordering::SeqCst);
        }
        *done_cb.0.lock().unwrap() = true;
        done_cb.1.notify_one();
    });

    unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };

    let (lock, cvar, ok_flag) = &*done;
    let finished = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(finished, SCK_WAIT_TIMEOUT).unwrap();
    let finished = guard;
    if !*finished {
        return Err(anyhow!("SCStream start did not complete within {:?}", SCK_WAIT_TIMEOUT));
    }
    if !ok_flag.load(Ordering::SeqCst) {
        let msg = err_slot.lock().unwrap().take().unwrap_or_else(|| "unknown".into());
        return Err(anyhow!("SCStream start error: {}", msg));
    }
    Ok(())
}
