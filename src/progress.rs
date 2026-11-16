//! In-flight HTTP fetch progress for the UI's progress bar. The download
//! loop in [`crate::fetch`] updates the state; the UI only reads it, so the
//! fetch layer stays free of UI dependencies and the UI never touches the
//! fetch machinery directly.

use parking_lot::Mutex;

/// FetchProgress snapshots the in-flight HTTP fetch for the UI's progress
/// bar. The download loop updates it; the UI only reads it.
#[derive(Clone, Debug)]
pub struct FetchProgress {
    /// human-readable phase, e.g. "updating SchemaStore cache"
    pub label: String,
    /// bytes downloaded so far
    pub got: u64,
    /// Content-Length when the server sent one
    pub total: Option<u64>,
}

static PROGRESS: Mutex<Option<ProgressState>> = Mutex::new(None);

#[derive(Clone, Debug)]
struct ProgressState {
    label: String,
    got: u64,
    total: Option<u64>,
}

/// progress snapshots the running fetch, if any.
pub fn progress() -> Option<FetchProgress> {
    PROGRESS.lock().as_ref().map(|s| FetchProgress {
        label: s.label.clone(),
        got: s.got,
        total: s.total,
    })
}

/// progress_label names the fetch phase for a location: SchemaStore URLs
/// are the cache update, everything else is a schema download.
fn progress_label(loc: &str) -> String {
    if loc.contains("schemastore.org") {
        "updating SchemaStore cache".to_string()
    } else {
        "fetching schema".to_string()
    }
}

/// progressBegin opens a fetch phase for `loc`; the state clears when the
/// matching [`ProgressGuard`] drops.
pub(crate) fn progress_begin(loc: &str) {
    *PROGRESS.lock() = Some(ProgressState {
        label: progress_label(loc),
        got: 0,
        total: None,
    });
}

pub(crate) fn progress_total(total: u64) {
    if let Some(s) = PROGRESS.lock().as_mut() {
        s.total = Some(total);
    }
}

pub(crate) fn progress_tick(got: u64) {
    if let Some(s) = PROGRESS.lock().as_mut() {
        s.got = got;
    }
}

pub(crate) fn progress_end() {
    *PROGRESS.lock() = None;
}

/// ProgressGuard clears the progress state on every exit path of a download.
pub(crate) struct ProgressGuard;

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        progress_end();
    }
}
