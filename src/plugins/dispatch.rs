//! Per-keystroke query dispatch.
//!
//! Stage 3 keeps this brutally simple: on every keystroke we call `query()`
//! on every loaded plugin sequentially, collect its results, merge, sort.
//! Stage 4 introduces per-plugin debounce, `AbortSignal` cancellation, and
//! incremental rendering.

use crate::plugins::result::{RankedResult, sort_merged};
use crate::plugins::runtime::LoadedPlugin;

/// Run `query(input)` against every plugin in `plugins`, merge results, sort.
///
/// A failing plugin logs to stderr and is dropped from this round's results;
/// the other plugins continue.
pub async fn dispatch(plugins: &[LoadedPlugin], input: &str) -> Vec<RankedResult> {
    let mut merged = Vec::new();
    let mut order = 0_usize;
    for plugin in plugins {
        match plugin.run_query(input).await {
            Ok(results) => {
                for result in results {
                    merged.push(RankedResult {
                        plugin_name: plugin.manifest.name.clone(),
                        result,
                        order,
                    });
                    order += 1;
                }
            }
            Err(err) => {
                eprintln!("plugins: {}: query failed: {err}", plugin.manifest.name);
            }
        }
    }
    sort_merged(merged)
}
