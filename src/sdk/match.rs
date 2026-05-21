//! Host implementation of the `highbeam:match` module.
//!
//! No capability — pure compute. Backed by `nucleo-matcher`. Items are
//! JS-opaque — the caller's `key` function extracts the haystack string, we
//! return the original value back attached to each match so plugins can stash
//! arbitrary shapes without serde round-tripping.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use rquickjs::{Array, Ctx, Function, Object, Result as JsResult, Value, module::ModuleDef};

pub struct MatchModule;

impl ModuleDef for MatchModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("fuzzy")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        let fuzzy = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, items: Array<'js>, query: String, opts: Value<'js>| -> JsResult<Array<'js>> {
                fuzzy_impl(&ctx, &items, &query, &opts)
            },
        )?;
        exports.export("fuzzy", fuzzy)?;
        Ok(())
    }
}

/// nucleo's raw score is unbounded; we normalise to 0..1 by clamping at this
/// ceiling so `opts.threshold` is a meaningful 0..1 bar.
const SCORE_CEILING: f64 = 256.0;

fn fuzzy_impl<'js>(ctx: &Ctx<'js>, items: &Array<'js>, query: &str, opts: &Value<'js>) -> JsResult<Array<'js>> {
    let opts_obj = opts.as_object();

    let key_fn: Option<Function<'js>> = opts_obj.and_then(|o| o.get("key").ok());
    let threshold: f64 = opts_obj
        .and_then(|o| o.get::<_, f64>("threshold").ok())
        .filter(|t| t.is_finite())
        .unwrap_or(0.0);
    let limit: Option<usize> = opts_obj
        .and_then(|o| o.get::<_, f64>("limit").ok())
        .filter(|l| l.is_finite() && *l > 0.0)
        .map(|l| {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let n = l as usize;
            n
        });

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

    // Pull (item, haystack) pairs out of the JS array once so we don't call
    // back into JS for every score round.
    let mut prepared: Vec<(Value<'js>, String)> = Vec::with_capacity(items.len());
    for item_res in items.iter::<Value<'js>>() {
        let item = item_res?;
        let key = if let Some(key_fn) = &key_fn {
            key_fn.call::<_, String>((item.clone(),))?
        } else {
            // Without `opts.key`: stringify the item itself so
            // `fuzzy(['a','b','c'], 'a')` works.
            match item.clone().into_string() {
                Some(s) => s.to_string()?,
                None => continue,
            }
        };
        prepared.push((item, key));
    }

    let mut scored: Vec<(Value<'js>, u32, Vec<u32>, String)> = Vec::with_capacity(prepared.len());
    for (item, key) in prepared {
        let haystack = Utf32String::from(key.as_str());
        let mut indices: Vec<u32> = Vec::new();
        if let Some(score) = pattern.indices(haystack.slice(..), &mut matcher, &mut indices) {
            if score == 0 {
                continue;
            }
            indices.sort_unstable();
            indices.dedup();
            scored.push((item, score, indices, key));
        }
    }

    scored.sort_by_key(|s| std::cmp::Reverse(s.1));
    if let Some(n) = limit {
        scored.truncate(n);
    }

    let out = Array::new(ctx.clone())?;
    // `continue` below skips threshold filtered entries — `enumerate()` would
    // leave gaps in the JS array index, so maintain it explicitly.
    #[allow(clippy::explicit_counter_loop)]
    {
        let mut out_idx = 0usize;
        for (item, score, indices, key) in scored {
            let normalized = f64::from(score) / SCORE_CEILING;
            let normalized = normalized.min(1.0);
            if normalized < threshold {
                continue;
            }
            let entry = Object::new(ctx.clone())?;
            entry.set("item", item)?;
            entry.set("score", normalized)?;
            entry.set("highlights", highlights_from_indices(ctx, &indices, &key)?)?;
            out.set(out_idx, entry)?;
            out_idx += 1;
        }
    }
    Ok(out)
}

/// Collapse a sorted `Vec<u32>` of UTF-32 char positions into contiguous
/// `[byteStart, byteEnd]` ranges so plugins can `slice()` the haystack
/// directly.
fn highlights_from_indices<'js>(ctx: &Ctx<'js>, indices: &[u32], haystack: &str) -> JsResult<Array<'js>> {
    let out = Array::new(ctx.clone())?;
    if indices.is_empty() {
        return Ok(out);
    }

    let mut char_to_byte: Vec<usize> = Vec::with_capacity(haystack.chars().count() + 1);
    for (byte_idx, _) in haystack.char_indices() {
        char_to_byte.push(byte_idx);
    }
    char_to_byte.push(haystack.len());

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = indices[0] as usize;
    let mut end = start;
    for &idx in &indices[1..] {
        let idx = idx as usize;
        if idx == end + 1 {
            end = idx;
        } else {
            ranges.push((start, end + 1));
            start = idx;
            end = idx;
        }
    }
    ranges.push((start, end + 1));

    for (out_idx, (s, e)) in ranges.into_iter().enumerate() {
        let byte_start = char_to_byte.get(s).copied().unwrap_or(haystack.len());
        let byte_end = char_to_byte.get(e).copied().unwrap_or(haystack.len());
        let pair = Array::new(ctx.clone())?;
        pair.set(0, byte_start)?;
        pair.set(1, byte_end)?;
        out.set(out_idx, pair)?;
    }
    Ok(out)
}
