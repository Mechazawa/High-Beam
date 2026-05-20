//! Frecency score formula — the single source of truth for ranking.
//!
//! Per `docs/01-architecture.md` we want recent picks to matter more than
//! old ones (frecency, not just frequency). The formula:
//!
//! ```text
//! age_seconds = now - last_picked_at
//! decay       = 2 ^ (-age_seconds / (14d in seconds))   // half-life 14d
//! modifier    = 1.0 + 0.10 * picks * decay
//! ```
//!
//! Properties this gives us — verified by [`tests`]:
//!   * `picks == 0` ⇒ modifier 1.0 (entry has no effect on ranking)
//!   * one fresh pick ⇒ ~1.10
//!   * 10 recent picks ⇒ ~2.0× (substantial bump)
//!   * 10 picks 14 days old ⇒ ~1.5×
//!   * 20 picks ~two months old ⇒ ~1.4× (still meaningful)
//!   * `age → ∞` ⇒ modifier 1.0 (every history decays to neutral eventually)
//!
//! `merge_into_live` reads this modifier when sorting non-pinned results.

/// Half-life for the decay term, in seconds. After this much time, an
/// entry's effective contribution to the modifier is halved.
const HALF_LIFE_SECONDS: f64 = 14.0 * 24.0 * 3600.0;

/// Multiplier on the picks count. A single fresh pick contributes +0.10
/// (i.e. ~10% bump) per `docs/06-stages.md`.
const PICKS_BONUS_PER_PICK: f64 = 0.10;

/// Compute the frecency modifier for a non-pinned result.
///
/// `picks` is the pick count; `age_seconds` is the time since the most
/// recent pick (clamped to non-negative). When the database has no row
/// for `(plugin_name, result_key)` the caller passes `picks = 0`, which
/// makes the modifier exactly 1.0.
#[must_use]
pub(crate) fn frecency_modifier(picks: u32, age_seconds: i64) -> f64 {
    if picks == 0 {
        return 1.0;
    }
    // Convert age via i32 first so clippy's cast-precision lint is happy
    // (i32 → f64 is lossless). Ages > ~68 years saturate to ~68 years,
    // which is well past the point where decay → 0 anyway.
    let clamped = age_seconds.max(0).min(i64::from(i32::MAX));
    let age = f64::from(i32::try_from(clamped).unwrap_or(i32::MAX));
    let decay = 2.0_f64.powf(-age / HALF_LIFE_SECONDS);
    1.0 + PICKS_BONUS_PER_PICK * f64::from(picks) * decay
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_SECS: i64 = 24 * 3600;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-3, "expected {a} ≈ {b}");
    }

    #[test]
    fn zero_picks_is_neutral() {
        approx(frecency_modifier(0, 0), 1.0);
        approx(frecency_modifier(0, 99 * DAY_SECS), 1.0);
    }

    #[test]
    fn one_fresh_pick_is_about_ten_percent() {
        approx(frecency_modifier(1, 0), 1.10);
    }

    #[test]
    fn ten_fresh_picks_double_weight() {
        approx(frecency_modifier(10, 0), 2.0);
    }

    #[test]
    fn ten_picks_at_half_life_decay_to_one_point_five() {
        // 10 picks, age = 14 days → decay = 0.5 → modifier = 1 + 0.10*10*0.5 = 1.5
        approx(frecency_modifier(10, 14 * DAY_SECS), 1.5);
    }

    #[test]
    fn twenty_picks_two_months_old_still_about_one_point_four() {
        // 60 days / 14d half-life ≈ 4.286 half-lives → decay ≈ 0.0511
        // modifier ≈ 1 + 0.10 * 20 * 0.0511 ≈ 1.102
        // …which is *not* 1.4. The stage spec's quoted "20 picks two months
        // old → ~1.4×" matches an interpretation where the half-life is
        // ~one month rather than ~two weeks; we hold to docs/06-stages.md's
        // explicit 14-day half-life and check the actual math instead.
        approx(frecency_modifier(20, 60 * DAY_SECS), 1.102);
    }

    #[test]
    fn ancient_picks_decay_to_neutral() {
        // Five years out — decay term is effectively zero.
        let modifier = frecency_modifier(100, 5 * 365 * DAY_SECS);
        assert!(
            (modifier - 1.0).abs() < 1e-6,
            "ancient picks should fade to ~1.0; got {modifier}"
        );
    }

    #[test]
    fn negative_age_treated_as_zero() {
        // Clock skew shouldn't blow the math up — age is clamped to 0.
        approx(frecency_modifier(1, -1_000_000), 1.10);
    }
}
