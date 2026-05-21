//! Per-keystroke query dispatch — streaming + per-plugin debounce + abort.
//!
//! Each plugin's `mpsc::Receiver` is tagged with its plugin name and merged
//! into the caller's single tx so yields render as they arrive. Scheduling
//! lives in `crate::app`; this module is just glue.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::frecency::{Snapshot, frecency_modifier, now_seconds};
use crate::plugins::result::{PluginResult, RankedResult};
use crate::plugins::runtime::LoadedPlugin;

/// Cap on a manifest's `debounceMs` — keeps a pathological plugin from
/// holding up an otherwise responsive UI.
pub(crate) const MAX_DEBOUNCE_MS: u64 = 2000;

/// One result, tagged with the plugin name that produced it.
#[derive(Debug)]
pub(crate) struct StreamedResult {
    pub plugin_name: String,
    pub result: PluginResult,
}

/// Spawn a per-plugin task for every loaded plugin (and the synchronous Core
/// built-in). Returns immediately; tasks honor `cancel` and drain when it
/// flips.
pub(crate) fn dispatch_streaming(
    plugins: &[Arc<LoadedPlugin>],
    input: &str,
    cancel: &CancellationToken,
    tx: &mpsc::UnboundedSender<StreamedResult>,
) {
    // Built-in Core emits synchronously, ahead of the JS pipeline.
    let plugin_names: Vec<String> = plugins.iter().map(|p| p.manifest.name.clone()).collect();
    for result in crate::plugins::builtin::core::query(input, &plugin_names) {
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
                // Abort the sleep on a new keystroke so we never spawn a query
                // whose result was already stale before it started.
                tokio::select! {
                    () = sleep(debounce) => {}
                    () = plugin_cancel.cancelled() => return,
                }
            }
            let mut rx_inner = plugin_arc.run_query_stream(&plugin_input, plugin_cancel);
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
/// Sort order:
///   1. Pinned first, sorted by `weight` desc — frecency doesn't apply
///      to pinned results.
///   2. Non-pinned next, sorted by `weight * frecency_modifier(picks, age)`
///      desc. `frecency = None` ⇒ modifier 1.0 ⇒ pure weight ordering.
///   3. Ties broken by insertion order (stable).
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
    // Nine rows max — per-yield re-sort is cheaper than maintaining a heap.
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

/// Effective sort key. Pinned results stay on pure `weight`; non-pinned get
/// the frecency modifier folded in.
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

/// Convenience wrapper using the system clock — the snapshot is per-query
/// but `now` is per-yield.
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
                icon: None,
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
        // (plugin=a, key=x) must NOT bump (plugin=b, key=x).
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
        // a:x carries the 5-pick bonus; b:x doesn't.
        assert_eq!(keys, ["x", "x"]);
    }
}
