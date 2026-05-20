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

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::plugins::result::{PluginResult, RankedResult};
use crate::plugins::runtime::LoadedPlugin;

/// Maximum a manifest's `debounceMs` can claim. Clamp keeps a pathological
/// plugin from holding up an otherwise responsive UI.
pub(crate) const MAX_DEBOUNCE_MS: u64 = 2000;

/// One result, tagged with the plugin name that produced it. The dispatcher
/// caller uses these to merge + re-sort + render.
pub(crate) struct StreamedResult {
    pub plugin_name: String,
    pub result: PluginResult,
}

/// Run `query(input)` against every plugin in `plugins`, with per-plugin
/// debounce, and stream each plugin's yields through `tx`. The function
/// returns immediately after spawning per-plugin tasks; it does NOT wait for
/// every plugin to finish. Per-plugin tasks honor `cancel` and drain when it
/// flips.
pub(crate) fn dispatch_streaming(
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

        tokio::spawn(async move {
            if !debounce.is_zero() {
                // Wait out the debounce; abort the sleep on a new keystroke
                // so we never spawn a query whose result was already stale
                // before it started.
                tokio::select! {
                    () = sleep(debounce) => {}
                    () = plugin_cancel.cancelled() => return,
                }
            }
            let mut rx_inner = plugin_arc.run_query_stream(&plugin_input, plugin_cancel);
            // Plugin's stream_query honors cancel directly; when cancelled it
            // drops its tx and we drop out of this loop naturally.
            while let Some(result) = rx_inner.recv().await {
                if tx_clone
                    .send(StreamedResult {
                        plugin_name: plugin_name.clone(),
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
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
pub(crate) fn merge_into_live(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::result::Action;

    fn streamed(key: &str, weight: f64, pinned: bool) -> StreamedResult {
        StreamedResult {
            plugin_name: "a".into(),
            result: PluginResult {
                key: key.into(),
                title: key.into(),
                subtitle: None,
                weight,
                pinned,
                action: Action::Copy { text: key.into() },
            },
        }
    }

    fn merge_all(items: Vec<StreamedResult>) -> Vec<String> {
        let mut live = Vec::new();
        let mut order = 0usize;
        for it in items {
            merge_into_live(&mut live, &mut order, it);
        }
        live.iter().map(|r| r.result.key.clone()).collect()
    }

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
        let keys = merge_all(vec![
            streamed("z", 1.0, false),
            streamed("p", 0.0, true),
            streamed("m", 10.0, false),
        ]);
        assert_eq!(keys, ["p", "m", "z"]);
    }

    #[test]
    fn merge_pinned_sorts_above_unpinned_regardless_of_weight() {
        let keys = merge_all(vec![
            streamed("high", 100.0, false),
            streamed("low-pinned", 0.0, true),
        ]);
        assert_eq!(keys, ["low-pinned", "high"]);
    }

    #[test]
    fn merge_unpinned_sorts_by_descending_weight() {
        let keys = merge_all(vec![
            streamed("low", 1.0, false),
            streamed("high", 10.0, false),
            streamed("mid", 5.0, false),
        ]);
        assert_eq!(keys, ["high", "mid", "low"]);
    }

    #[test]
    fn merge_equal_weight_ties_break_by_insertion_order() {
        let keys = merge_all(vec![
            streamed("first", 5.0, false),
            streamed("second", 5.0, false),
            streamed("third", 5.0, false),
        ]);
        assert_eq!(keys, ["first", "second", "third"]);
    }

    #[test]
    fn merge_assigns_unique_order_per_yield() {
        let mut live = Vec::new();
        let mut order = 0usize;
        for k in ["a", "b", "c"] {
            merge_into_live(&mut live, &mut order, streamed(k, 0.0, false));
        }
        assert_eq!(order, 3);
        let orders: Vec<_> = live.iter().map(|r| r.order).collect();
        assert_eq!(orders, [0, 1, 2]);
    }
}
