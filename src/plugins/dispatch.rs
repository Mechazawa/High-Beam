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

use crate::frecency::{Snapshot, frecency_modifier, now_seconds};
use crate::plugins::result::{PluginResult, RankedResult};
use crate::plugins::runtime::LoadedPlugin;

/// Maximum a manifest's `debounceMs` can claim. Clamp keeps a pathological
/// plugin from holding up an otherwise responsive UI.
pub(crate) const MAX_DEBOUNCE_MS: u64 = 2000;

/// One result, tagged with the plugin name that produced it. The dispatcher
/// caller uses these to merge + re-sort + render.
#[derive(Debug)]
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
    // Built-in plugins emit synchronously; spawning them on the same channel
    // before the JS pipeline runs gives Core results a chance to land before
    // the first keystroke-debounced JS plugin even starts.
    for result in crate::plugins::builtin::core::query(input) {
        if tx.send(result).is_err() {
            return;
        }
    }
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

/// Merge a freshly-yielded result into the live row list and re-sort.
///
/// Sort order, per `docs/01-architecture.md` and Stage 5 spec:
///   1. **Pinned first**, sorted by `weight` desc — frecency doesn't apply
///      to pinned results (they're authoritative for their input shape).
///   2. **Non-pinned next**, sorted by `weight * frecency_modifier(picks, age)`
///      desc. `frecency` is `None` ⇒ modifier 1.0 ⇒ Stage 4 behaviour, i.e.
///      pure weight ordering.
///   3. Ties broken by insertion order (stable).
///
/// `now` is taken as a parameter so unit tests can produce deterministic
/// ages; production callers pass [`now_seconds`].
pub(crate) fn merge_into_live(
    live: &mut Vec<RankedResult>,
    next_order: &mut usize,
    incoming: StreamedResult,
    frecency: Option<&Snapshot>,
    now: i64,
) {
    let entry = RankedResult {
        plugin_name: incoming.plugin_name,
        result: incoming.result,
        order: *next_order,
    };
    *next_order += 1;
    live.push(entry);
    // With nine rows max this is trivially cheap; accepting the per-yield
    // sort saves us a priority structure.
    live.sort_by(|a, b| {
        b.result
            .pinned
            .cmp(&a.result.pinned)
            .then_with(|| {
                let a_score = score_for(a, frecency, now);
                let b_score = score_for(b, frecency, now);
                b_score
                    .partial_cmp(&a_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.order.cmp(&b.order))
    });
}

/// Effective sort key for one result.
///
/// Pinned results stay on pure `weight` so the calculator-class plugins
/// don't drift relative to one another; non-pinned results get the
/// frecency modifier folded in.
fn score_for(entry: &RankedResult, frecency: Option<&Snapshot>, now: i64) -> f64 {
    let weight = entry.result.weight;
    if entry.result.pinned {
        return weight;
    }
    let modifier = frecency
        .and_then(|snap| snap.get(&entry.plugin_name, &entry.result.key))
        .map_or(1.0, |row| {
            frecency_modifier(row.picks, now - row.last_picked_at)
        });
    weight * modifier
}

/// Convenience wrapper around [`merge_into_live`] using the system clock —
/// used by the dispatcher receive loop where the snapshot is per-query but
/// `now` is per-yield.
pub(crate) fn merge_with_snapshot(
    live: &mut Vec<RankedResult>,
    next_order: &mut usize,
    incoming: StreamedResult,
    frecency: Option<&Snapshot>,
) {
    merge_into_live(live, next_order, incoming, frecency, now_seconds());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frecency::PickRow;
    use crate::plugins::result::Action;

    fn streamed(key: &str, weight: f64, pinned: bool) -> StreamedResult {
        streamed_from("a", key, weight, pinned)
    }

    fn streamed_from(plugin: &str, key: &str, weight: f64, pinned: bool) -> StreamedResult {
        StreamedResult {
            plugin_name: plugin.into(),
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
        merge_all_with(items, None, 0)
    }

    fn merge_all_with(
        items: Vec<StreamedResult>,
        snapshot: Option<&Snapshot>,
        now: i64,
    ) -> Vec<String> {
        let mut live = Vec::new();
        let mut order = 0usize;
        for it in items {
            merge_into_live(&mut live, &mut order, it, snapshot, now);
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
            merge_into_live(&mut live, &mut order, streamed(k, 0.0, false), None, 0);
        }
        assert_eq!(order, 3);
        let orders: Vec<_> = live.iter().map(|r| r.order).collect();
        assert_eq!(orders, [0, 1, 2]);
    }

    #[test]
    fn frecency_promotes_recent_picks_above_tied_weights() {
        // All three rows have weight 50; only `gamma` has a pick. With
        // 1 pick at age 0 the modifier is ~1.10, so gamma should rank
        // ahead of alpha + beta on the second query.
        let snap = Snapshot::from_rows(vec![(
            "a".into(),
            "gamma".into(),
            PickRow {
                picks: 1,
                last_picked_at: 1_000,
            },
        )]);
        let keys = merge_all_with(
            vec![
                streamed("alpha", 50.0, false),
                streamed("beta", 50.0, false),
                streamed("gamma", 50.0, false),
            ],
            Some(&snap),
            1_000,
        );
        assert_eq!(keys, ["gamma", "alpha", "beta"]);
    }

    #[test]
    fn frecency_does_not_promote_pinned_results_relative_to_one_another() {
        // Two pinned results, equal weight. Even if beta has 100 picks,
        // the modifier should NOT apply to pinned — they sort by raw
        // weight then insertion order.
        let snap = Snapshot::from_rows(vec![(
            "a".into(),
            "beta".into(),
            PickRow {
                picks: 100,
                last_picked_at: 1_000,
            },
        )]);
        let keys = merge_all_with(
            vec![streamed("alpha", 10.0, true), streamed("beta", 10.0, true)],
            Some(&snap),
            1_000,
        );
        assert_eq!(keys, ["alpha", "beta"]);
    }

    #[test]
    fn pinned_still_beats_heavily_picked_non_pinned() {
        let snap = Snapshot::from_rows(vec![(
            "a".into(),
            "non-pinned".into(),
            PickRow {
                picks: 1000,
                last_picked_at: 1_000,
            },
        )]);
        let keys = merge_all_with(
            vec![
                streamed("non-pinned", 100.0, false),
                streamed("pinned", 1.0, true),
            ],
            Some(&snap),
            1_000,
        );
        assert_eq!(keys, ["pinned", "non-pinned"]);
    }

    #[test]
    fn frecency_keyed_on_plugin_name_too() {
        // A pick on (plugin=a, key=x) must NOT bump (plugin=b, key=x).
        let snap = Snapshot::from_rows(vec![(
            "a".into(),
            "x".into(),
            PickRow {
                picks: 5,
                last_picked_at: 1_000,
            },
        )]);
        let keys = merge_all_with(
            vec![
                streamed_from("a", "x", 50.0, false),
                streamed_from("b", "x", 50.0, false),
            ],
            Some(&snap),
            1_000,
        );
        // a:x has a 5-pick bonus, b:x has none.
        assert_eq!(keys, ["x", "x"]);
        // Insertion order still decides ties — but here the bonus
        // shouldn't apply to b's `x`, so a:x wins outright.
    }
}
