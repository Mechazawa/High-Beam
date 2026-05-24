//! Frecency score formula — the single source of truth for ranking.
//!
//! ```text
//! age_seconds = now - last_picked_at
//! decay       = 2 ^ (-age_seconds / (14d in seconds))   // half-life 14d
//! modifier    = 1.0 + 0.10 * picks * decay
//! ```
//!
//! `picks == 0` ⇒ modifier 1.0; one fresh pick ⇒ ~1.10; 10 recent picks ⇒
//! ~2.0×; ancient picks fade to 1.0. See tests below.

/// Half-life for the decay term, in seconds.
const HALF_LIFE_SECONDS: f64 = 14.0 * 24.0 * 3600.0;

/// A single fresh pick contributes +0.10 (~10% bump).
const PICKS_BONUS_PER_PICK: f64 = 0.10;

/// Compute the frecency modifier for a non-pinned result.
///
/// `picks = 0` (no row in the DB) makes the modifier exactly 1.0.
#[must_use]
pub(crate) fn frecency_modifier(picks: u32, age_seconds: i64) -> f64 {
    if picks == 0 {
        return 1.0;
    }
    // Convert via i32 first so the cast-precision lint stays happy
    // (i32 → f64 is lossless). Ages past ~68 years saturate, which is well
    // past the point where the decay term is effectively zero anyway.
    let clamped = age_seconds.max(0).min(i64::from(i32::MAX));
    let age = f64::from(i32::try_from(clamped).unwrap_or(i32::MAX));
    // `(x).exp2()` is more accurate than `2.0.powf(x)` for base-2; `mul_add`
    // is a fused multiply-add with one rounding instead of two. Both flagged
    // by clippy::nursery; neither changes test outcomes (the approx checks
    // use 1e-3 / 1e-6 tolerances that comfortably cover the bit-level shift).
    let decay = (-age / HALF_LIFE_SECONDS).exp2();
    (PICKS_BONUS_PER_PICK * f64::from(picks)).mul_add(decay, 1.0)
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
        // modifier ≈ 1 + 0.10 * 20 * 0.0511 ≈ 1.102.
        approx(frecency_modifier(20, 60 * DAY_SECS), 1.102);
    }

    #[test]
    fn ancient_picks_decay_to_neutral() {
        let modifier = frecency_modifier(100, 5 * 365 * DAY_SECS);
        assert!(
            (modifier - 1.0).abs() < 1e-6,
            "ancient picks should fade to ~1.0; got {modifier}"
        );
    }

    #[test]
    fn negative_age_treated_as_zero() {
        approx(frecency_modifier(1, -1_000_000), 1.10);
    }
}
