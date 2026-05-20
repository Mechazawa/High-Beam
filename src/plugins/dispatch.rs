//! Per-keystroke query dispatch — streaming + per-plugin debounce + abort.
//!
//! Stage 3's blocking "collect each plugin's Vec, sort, render once" model is
//! gone. Stage 4 model:
//!   * Each plugin gets its own `mpsc::Receiver` from
//!     [`LoadedPlugin::run_query_stream`]. We merge those receivers into a
//!     single tagged stream so the renderer can react to each yielded result
//!     immediately.
//!   * Per-plugin debounce is enforced *before* `run_query_stream` is called.
//!   * Cancellation: every dispatch round produces a fresh root
//!     [`CancellationToken`]; when a new keystroke arrives, the previous
//!     token's `cancel()` is called and the old per-plugin streams drain
//!     quickly.
//!
//! The dispatcher itself is intentionally just glue. The dispatcher caller
//! (`crate::app`) owns the scheduling — they get a token, ask the dispatcher
//! to run, and observe yielded results plus stream-complete signals.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::plugins::result::{PluginResult, RankedResult};
use crate::plugins::runtime::LoadedPlugin;

/// Maximum a manifest's `debounceMs` can claim. Clamp keeps a pathological
/// plugin from holding up an otherwise responsive UI.
pub const MAX_DEBOUNCE_MS: u64 = 2000;

/// One result, tagged with the plugin name and the dispatch id that
/// produced it. The dispatcher caller uses these to merge + re-sort + render.
pub struct StreamedResult {
    pub plugin_name: String,
    pub result: PluginResult,
}

/// Run `query(input)` against every plugin in `plugins`, with per-plugin
/// debounce, and stream each plugin's yields through `tx`. The function
/// returns immediately after spawning per-plugin tasks; it does NOT wait for
/// every plugin to finish. Per-plugin tasks honor `cancel` and drain when it
/// flips.
///
/// `dispatch_id` is opaque to the dispatcher — pass whatever monotonic id
/// the caller uses to drop stale yields.
pub fn dispatch_streaming(
    plugins: &[Arc<LoadedPlugin>],
    input: &str,
    cancel: &CancellationToken,
    tx: &mpsc::UnboundedSender<StreamedResult>,
) {
    for plugin in plugins {
        let plugin_name = plugin.manifest.name.clone();
        let debounce = clamp_debounce(plugin.manifest.debounce_ms);
        let plugin_cancel = cancel.child_token();
        let plugin_arc = Arc::clone(plugin);
        let plugin_input = input.to_owned();
        let tx_clone = tx.clone();

        if debounce.is_zero() {
            // Immediate dispatch — open the stream now, forward each yield.
            let cancel_for_task = plugin_cancel.clone();
            tokio::spawn(async move {
                let mut rx_inner =
                    plugin_arc.run_query_stream(&plugin_input, cancel_for_task.clone());
                forward_stream(plugin_name, &mut rx_inner, tx_clone, cancel_for_task).await;
            });
        } else {
            // Debounced dispatch — sleep first, abort sleep if a new
            // keystroke arrives mid-debounce.
            let cancel_for_sleep = plugin_cancel.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = sleep(debounce) => {
                        let mut rx_inner = plugin_arc
                            .run_query_stream(&plugin_input, cancel_for_sleep.clone());
                        forward_stream(plugin_name, &mut rx_inner, tx_clone, cancel_for_sleep).await;
                    }
                    () = cancel_for_sleep.cancelled() => {
                        // Debounce period was cut short by a new keystroke;
                        // skip dispatch entirely.
                    }
                }
            });
        }
    }
}

/// Forward each result from a per-plugin receiver into the merged channel.
/// Terminates on cancel or when the plugin's stream closes.
async fn forward_stream(
    plugin_name: String,
    rx: &mut mpsc::UnboundedReceiver<PluginResult>,
    tx: mpsc::UnboundedSender<StreamedResult>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            r = rx.recv() => match r {
                Some(result) => {
                    if tx.send(StreamedResult {
                        plugin_name: plugin_name.clone(),
                        result,
                    }).is_err() {
                        return;
                    }
                }
                None => return,
            }
        }
    }
}

fn clamp_debounce(ms: u64) -> Duration {
    Duration::from_millis(ms.min(MAX_DEBOUNCE_MS))
}

/// Merge a freshly-yielded result into the live row list (which is sorted
/// pinned-first by weight, then by insertion order for non-pinned ties).
///
/// Mutates `live` in place. The caller pushes the resulting state back to
/// the UI.
pub fn merge_into_live(
    live: &mut Vec<RankedResult>,
    next_order: &mut usize,
    incoming: StreamedResult,
) {
    let entry = RankedResult {
        plugin_name: incoming.plugin_name,
        result: incoming.result,
        order: *next_order,
    };
    *next_order += 1;
    live.push(entry);
    // Re-sort the slice. With nine rows max this is trivially cheap; we
    // accept the cost in exchange for not maintaining a separate priority
    // structure. Stage 5 (frecency scoring) will revisit this if profiling
    // turns up anything.
    live.sort_by(|a, b| {
        b.result
            .pinned
            .cmp(&a.result.pinned)
            .then_with(|| {
                b.result
                    .weight
                    .partial_cmp(&a.result.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.order.cmp(&b.order))
    });
}

/// Bookkeeping for per-plugin debounce scheduling. The app layer keeps one
/// of these per loaded plugin and consults [`Schedule::should_dispatch_now`]
/// (immediate) versus [`Schedule::cancel_pending`] (debounced).
pub struct Schedule {
    pending: HashMap<String, CancellationToken>,
}

impl Schedule {
    /// Empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Cancel any pending debounced dispatch for `plugin_name`. Idempotent.
    pub fn cancel_pending(&mut self, plugin_name: &str) {
        if let Some(token) = self.pending.remove(plugin_name) {
            token.cancel();
        }
    }

    /// Register a token under `plugin_name`. The caller should pass the same
    /// token to `dispatch_streaming` so a follow-up call to `cancel_pending`
    /// aborts both the sleep and any subsequently-spawned query.
    pub fn register_pending(&mut self, plugin_name: String, token: CancellationToken) {
        self.pending.insert(plugin_name, token);
    }

    /// Drop all pending tokens without cancelling — used when the dispatch
    /// has actually fired and we want to recycle the schedule slot.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_debounce_caps_at_max() {
        assert_eq!(clamp_debounce(0), Duration::from_millis(0));
        assert_eq!(clamp_debounce(500), Duration::from_millis(500));
        assert_eq!(
            clamp_debounce(MAX_DEBOUNCE_MS),
            Duration::from_millis(MAX_DEBOUNCE_MS)
        );
        assert_eq!(
            clamp_debounce(60_000),
            Duration::from_millis(MAX_DEBOUNCE_MS)
        );
    }

    #[test]
    fn merge_orders_pinned_then_weight_then_insertion() {
        let mut live = Vec::new();
        let mut order = 0usize;
        merge_into_live(
            &mut live,
            &mut order,
            StreamedResult {
                plugin_name: "a".into(),
                result: PluginResult {
                    key: "z".into(),
                    title: "z".into(),
                    subtitle: None,
                    weight: 1.0,
                    pinned: false,
                    action: crate::plugins::result::Action::Copy { text: "z".into() },
                },
            },
        );
        merge_into_live(
            &mut live,
            &mut order,
            StreamedResult {
                plugin_name: "a".into(),
                result: PluginResult {
                    key: "p".into(),
                    title: "p".into(),
                    subtitle: None,
                    weight: 0.0,
                    pinned: true,
                    action: crate::plugins::result::Action::Copy { text: "p".into() },
                },
            },
        );
        merge_into_live(
            &mut live,
            &mut order,
            StreamedResult {
                plugin_name: "a".into(),
                result: PluginResult {
                    key: "m".into(),
                    title: "m".into(),
                    subtitle: None,
                    weight: 10.0,
                    pinned: false,
                    action: crate::plugins::result::Action::Copy { text: "m".into() },
                },
            },
        );
        let keys: Vec<_> = live.iter().map(|r| r.result.key.clone()).collect();
        assert_eq!(keys, ["p", "m", "z"]);
    }

    #[test]
    fn schedule_cancel_pending_fires_token() {
        let mut sched = Schedule::new();
        let token = CancellationToken::new();
        sched.register_pending("p".into(), token.clone());
        assert!(!token.is_cancelled());
        sched.cancel_pending("p");
        assert!(token.is_cancelled());
        // Idempotent.
        sched.cancel_pending("p");
    }
}
