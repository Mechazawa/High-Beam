//! Per-keystroke query dispatch: kick off the streaming pipeline and merge
//! yields back into the launcher's row model as they arrive.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ModelRc, VecModel};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::QueryWindow;
use crate::frecency::{self, Snapshot};
use crate::logging::LogErr;
use crate::plugins::dispatch::{self, StreamedResult};
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::LoadedPlugin;
use crate::ui::ResultRow;
use crate::window;

/// Start a fresh query: clear the row list, kick off the streaming dispatch,
/// and spawn a receiver task that merges yields into the UI as they arrive.
pub(super) fn handle_query(
    id: u64,
    input: &str,
    plugins: &[Arc<LoadedPlugin>],
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
    frecency_snapshot: Option<Snapshot>,
) -> CancellationToken {
    let cancel = CancellationToken::new();

    let weak_reset = weak.clone();
    // `invoke_from_event_loop` only returns Err once the event loop has
    // exited — i.e. the daemon is shutting down. At that point dropping the
    // UI update is exactly what we want; `log_debug` keeps a breadcrumb
    // around for anyone debugging shutdown ordering.
    slint::invoke_from_event_loop(move || {
        if let Some(w) = weak_reset.upgrade() {
            w.set_results(ModelRc::new(VecModel::from(Vec::<ResultRow>::new())));
            w.set_selected_index(0);
        }
    })
    .log_debug("query: post reset to event loop");

    let (yield_tx, mut yield_rx) = mpsc::unbounded_channel::<StreamedResult>();
    dispatch::dispatch_streaming(plugins, input, &cancel, &yield_tx);

    let cancel_for_recv = cancel.clone();
    tokio::spawn(async move {
        let mut live: Vec<RankedResult> = Vec::new();
        let mut order: usize = 0;

        loop {
            tokio::select! {
                () = cancel_for_recv.cancelled() => break,
                next = yield_rx.recv() => match next {
                    Some(streamed) => {
                        if id < latest_id.load(Ordering::Relaxed) {
                            continue;
                        }
                        dispatch::merge_into_live(
                            &mut live,
                            &mut order,
                            streamed,
                            frecency_snapshot.as_ref(),
                            frecency::now_seconds(),
                        );

                        if let Ok(mut slot) = latest.lock() {
                            slot.clone_from(&live);
                        }
                        let snapshot = live.clone();
                        let weak = weak.clone();
                        slint::invoke_from_event_loop(move || {
                            if let Some(w) = weak.upgrade() {
                                render_results(&w, &snapshot);
                            }
                        })
                        .log_debug("query: post results render to event loop");
                    }
                    None => break,
                }
            }
        }
    });

    cancel
}

/// Push the dispatch results into the window's row model.
///
/// Preserves `selected-index` when the previously-selected row still exists;
/// otherwise resets to 0. Icons are decoded on this thread because
/// `slint::Image` isn't trivially constructed off-thread; the per-yield cost
/// is bounded by `max_rows` (9).
pub(super) fn render_results(window: &QueryWindow, results: &[RankedResult]) {
    let previously_selected = window.get_selected_index();
    let rows: Vec<ResultRow> = results
        .iter()
        .map(|r| {
            let icon_spec = r.result.icon.as_deref();
            let has_icon = is_renderable_icon(icon_spec);
            ResultRow {
                key: r.composite_key().into(),
                title: r.result.title.clone().into(),
                subtitle: r.result.subtitle.clone().unwrap_or_default().into(),
                has_subtitle: r.result.subtitle.is_some(),
                icon: window::decode_icon(icon_spec),
                has_icon,
                alt_title: r.result.alt_title.clone().unwrap_or_default().into(),
                has_alt_title: r.result.alt_title.is_some(),
                alt_subtitle: r.result.alt_subtitle.clone().unwrap_or_default().into(),
                has_alt_subtitle: r.result.alt_subtitle.is_some(),
            }
        })
        .collect();
    let row_count = i32::try_from(rows.len()).unwrap_or(i32::MAX);
    window.set_results(ModelRc::new(VecModel::from(rows)));

    if previously_selected >= row_count || previously_selected < 0 {
        window.set_selected_index(0);
    }
}

/// Anything that isn't a base64 data URI is treated as "no icon" so the row
/// renders the muted placeholder rather than appearing to load nothing.
fn is_renderable_icon(spec: Option<&str>) -> bool {
    spec.is_some_and(|s| s.starts_with("data:") && s.contains(";base64,"))
}
