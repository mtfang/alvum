//! SCContentFilter construction and the per-process filter config slot.
//!
//! Splitting the SCK filter rules from the stream lifecycle keeps the
//! ObjC bridging churn isolated from the high-level "what apps do we
//! want to capture" decision tree. Both pre-start config (via
//! [`configure`]) and runtime queries (via [`current_config`]) live
//! here, alongside [`build_filter`] which materialises the chosen
//! config into an `SCContentFilter`.

use anyhow::Result;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_foundation::NSArray;
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCWindow,
};
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

/// Which apps the SCK content filter should let through.
///
/// - `Exclude { ... }` (default) — capture everything except matching apps.
///   Empty lists = open world (capture all).
/// - `Include { ... }` — whitelist mode: capture ONLY matching apps. Empty
///   lists with Include is a degenerate "capture nothing" configuration;
///   `build_filter` logs a warning and falls back to open-world so the
///   daemon doesn't silently record nothing.
#[derive(Debug, Clone)]
pub enum AppFilter {
    Exclude { names: Vec<String>, bundle_ids: Vec<String> },
    Include { names: Vec<String>, bundle_ids: Vec<String> },
}

impl Default for AppFilter {
    fn default() -> Self {
        AppFilter::Exclude { names: Vec::new(), bundle_ids: Vec::new() }
    }
}

/// Pre-start configuration for the shared SCK stream. Set via
/// [`configure`] before [`crate::ensure_started`] is first called.
#[derive(Debug, Clone, Default)]
pub struct SharedStreamConfig {
    pub filter: AppFilter,
}

static FILTER_CONFIG: OnceLock<Mutex<SharedStreamConfig>> = OnceLock::new();

/// Provide the filter config that [`crate::ensure_started`] will use on first
/// start. Safe to call multiple times before start; last-writer-wins.
/// Idempotent after start, but the filter is not reshaped at runtime —
/// callers that need a live reshape should call `sync_active_display` or
/// trigger a display swap.
pub fn configure(cfg: SharedStreamConfig) {
    let slot = FILTER_CONFIG.get_or_init(|| Mutex::new(SharedStreamConfig::default()));
    *slot.lock().unwrap() = cfg;
}

pub(crate) fn current_config() -> SharedStreamConfig {
    FILTER_CONFIG
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

#[doc(hidden)]
pub fn snapshot_config_for_test() -> SharedStreamConfig {
    current_config()
}

/// Pure rule-matching helper used by both include and exclude filter modes.
/// Given name/bundle rule lists and a snapshot of (app_name, bundle_id)
/// tuples, return the indices of matching apps. Name match is
/// case-insensitive; bundle match is exact. Names and bundle IDs are
/// OR'd — an app matching either list is a hit. Each matching app
/// appears exactly once in the result (no duplicate indices).
fn match_apps_by_rules(
    names: &[String],
    bundle_ids: &[String],
    apps: &[(String, String)],
) -> Vec<usize> {
    let names_lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut hits: Vec<usize> = Vec::new();
    for (i, (name, bundle)) in apps.iter().enumerate() {
        let name_hit = names_lower.iter().any(|n| n == &name.to_lowercase());
        let bundle_hit = bundle_ids.iter().any(|b| b == bundle);
        if name_hit || bundle_hit {
            hits.push(i);
        }
    }
    hits
}

/// Construct an SCContentFilter according to `cfg`. Returns the
/// wide-open `excludingWindows` filter when no rules apply or no apps
/// match — so a misconfigured exclude/include list can't silently
/// capture nothing.
pub(crate) fn build_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
    cfg: &SharedStreamConfig,
) -> Result<Retained<SCContentFilter>> {
    let empty_windows: Retained<NSArray<SCWindow>> = NSArray::new();

    // Early-exit open-world path: default Exclude with no rules → the
    // existing wide-open filter, no app enumeration needed.
    if let AppFilter::Exclude { names, bundle_ids } = &cfg.filter {
        if names.is_empty() && bundle_ids.is_empty() {
            return Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    display,
                    &empty_windows,
                )
            });
        }
    }

    let apps = unsafe { content.applications() };
    let mut tuples: Vec<(String, String)> = Vec::with_capacity(apps.count());
    let mut app_vec: Vec<Retained<SCRunningApplication>> = Vec::with_capacity(apps.count());
    for i in 0..apps.count() {
        let app = apps.objectAtIndex(i);
        let name = unsafe { app.applicationName() }.to_string();
        let bundle = unsafe { app.bundleIdentifier() }.to_string();
        tuples.push((name, bundle));
        app_vec.push(app);
    }

    let (names, bundle_ids, is_include) = match &cfg.filter {
        AppFilter::Exclude { names, bundle_ids } => (names, bundle_ids, false),
        AppFilter::Include { names, bundle_ids } => {
            if names.is_empty() && bundle_ids.is_empty() {
                warn!(
                    "AppFilter::Include with empty rules = capture-nothing; \
                     falling back to open world"
                );
                return Ok(unsafe {
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        display,
                        &empty_windows,
                    )
                });
            }
            (names, bundle_ids, true)
        }
    };

    let indices = match_apps_by_rules(names, bundle_ids, &tuples);
    if indices.is_empty() {
        warn!(
            names = ?names,
            bundles = ?bundle_ids,
            mode = if is_include { "include" } else { "exclude" },
            "no running apps matched SCK filter rules; falling back to open world"
        );
        return Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &empty_windows,
            )
        });
    }

    let matched_refs: Vec<&SCRunningApplication> =
        indices.iter().map(|&i| app_vec[i].as_ref()).collect();
    let matched_array: Retained<NSArray<SCRunningApplication>> =
        NSArray::from_slice(&matched_refs);

    let matched_names: Vec<&String> = indices.iter().map(|&i| &tuples[i].0).collect();
    if is_include {
        info!(included = ?matched_names, "SCK filter including only");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    } else {
        info!(excluded = ?matched_names, "SCK filter excluding apps");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_apps_empty_rules_returns_empty() {
        let apps = vec![("Music".into(), "com.apple.Music".into())];
        let idx = match_apps_by_rules(&[], &[], &apps);
        assert!(idx.is_empty());
    }

    #[test]
    fn match_apps_by_name_case_insensitive() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Safari".into(), "com.apple.Safari".into()),
        ];
        let idx = match_apps_by_rules(&["music".to_string()], &[], &apps);
        assert_eq!(idx, vec![0]);
    }

    #[test]
    fn match_apps_by_bundle_id() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Spotify".into(), "com.spotify.client".into()),
        ];
        let idx = match_apps_by_rules(&[], &["com.spotify.client".to_string()], &apps);
        assert_eq!(idx, vec![1]);
    }

    #[test]
    fn match_apps_by_name_and_bundle_unions() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Spotify".into(), "com.spotify.client".into()),
            ("Safari".into(), "com.apple.Safari".into()),
        ];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.spotify.client".to_string()],
            &apps,
        );
        assert_eq!(idx, vec![0, 1]);
    }

    #[test]
    fn match_apps_no_match_returns_empty() {
        let apps = vec![("Safari".into(), "com.apple.Safari".into())];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.apple.Music".to_string()],
            &apps,
        );
        assert!(idx.is_empty());
    }

    #[test]
    fn match_apps_deduplicates_when_name_and_bundle_both_hit_same_index() {
        let apps = vec![("Music".into(), "com.apple.Music".into())];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.apple.Music".to_string()],
            &apps,
        );
        assert_eq!(idx, vec![0], "one app should yield one index even if both rules match");
    }
}
