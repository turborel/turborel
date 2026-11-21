//! \[DESIGN\] Conjuncts on different columns combined by exponential
//! backoff (RULES.md D382): of their selectivities, ascending, s1 x
//! s2^(1/2) x s3^(1/4) x s4^(1/8), the rest ignored (Sack, "Cardinality
//! estimation done right: index-based join sampling", 2014; the
//! combination of correlated conjuncts commercial systems use). Taking
//! conjuncts as independent multiplies their selectivities, which is
//! right only if their columns are, and conjuncts written together are
//! seldom of independent columns: a city and its country, a model and
//! its maker. The most selective conjunct counts whole, each next one
//! less, so an estimate errs between independence and the most selective
//! alone, never far below the truth, where independence errs below it by
//! the product of every correlation.

/// \[DESIGN\] The most conjuncts backoff weighs: a fifth would count by
/// its sixteenth root, too little to tell.
pub(super) const WEIGHED: usize = 4;

/// The selectivities `shares`, of conjuncts on different columns,
/// combined: none is one.
pub(super) fn combined(shares: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = shares.iter().map(|s| s.clamp(0.0, 1.0)).collect();
    sorted.sort_by(f64::total_cmp);
    let mut root = 1.0_f64;
    let mut kept = 1.0_f64;
    for share in sorted.into_iter().take(WEIGHED) {
        kept *= share.powf(root);
        root /= 2.0_f64;
    }
    kept
}

// Unit tests: the order and weights of the factors (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::combined;

    /// Verifies: [DESIGN] RULES.md D382, backoff: the most selective
    /// conjunct counts whole, the next by its square root, the third its
    /// fourth root and the fourth its eighth, whatever order they are
    /// given in; a fifth is not weighed; none is one; one is itself.
    #[test]
    fn the_most_selective_counts_whole_and_the_rest_less() {
        assert_eq!(combined(&[]), 1.0_f64);
        assert_eq!(combined(&[0.3_f64]), 0.3_f64);
        let want = 0.01_f64 * 0.04_f64.sqrt() * 0.16_f64.powf(0.25_f64) * 0.25_f64.powf(0.125_f64);
        for shares in [[0.01_f64, 0.04_f64, 0.16_f64, 0.25_f64, 0.5_f64], [0.5_f64, 0.25_f64, 0.16_f64, 0.04_f64, 0.01_f64]] {
            assert!((combined(&shares) - want).abs() < 1e-12_f64, "{shares:?}");
        }
        assert!(combined(&[0.1_f64, 0.1_f64]) > 0.1_f64 * 0.1_f64, "above independence");
        assert!(combined(&[0.1_f64, 0.1_f64]) < 0.1_f64, "below the most selective alone");
    }
}
