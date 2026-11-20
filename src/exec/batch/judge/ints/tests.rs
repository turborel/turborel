//! Unit tests of an exact constant folded to a column's scale (`ints.rs`).

use super::*;
use crate::value::{CompOp, Decimal};

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// Whether `u op c` is true of units `u` as `bound` judges them.
fn by_bound(bound: Bound, u: i64) -> bool {
    match bound {
        Bound::Of(op, c) => holds(op, Some(u.cmp(&c))),
        Bound::All => true,
        Bound::None => false,
    }
}

/// Verifies: [DESIGN] RULES.md D354 [X3.135-1986 §5.11 GR4] a constant of
/// more fractional digits than the column's scale folds to a bound of
/// its units: `<` and `>=` by the ceiling, `<=` and `>` by the floor, `=`
/// true of none and `<>` of all unless the scale divides it; each pair
/// below differs in one condition alone, the operator or the remainder.
#[test]
fn constants_fold_to_bounds_of_the_scale() {
    let cases = [
        // 1.5 at scale 0: floor 1, ceiling 2.
        (0, CompOp::Lt, exact(15, 1), Bound::Of(CompOp::Lt, 2)),
        (0, CompOp::Le, exact(15, 1), Bound::Of(CompOp::Le, 1)),
        (0, CompOp::Gt, exact(15, 1), Bound::Of(CompOp::Gt, 1)),
        (0, CompOp::Ge, exact(15, 1), Bound::Of(CompOp::Ge, 2)),
        (0, CompOp::Eq, exact(15, 1), Bound::None),
        (0, CompOp::Ne, exact(15, 1), Bound::All),
        // 2.0 at scale 0: divided, so floor and ceiling are 2.
        (0, CompOp::Eq, exact(20, 1), Bound::Of(CompOp::Eq, 2)),
        (0, CompOp::Ne, exact(20, 1), Bound::Of(CompOp::Ne, 2)),
        (0, CompOp::Lt, exact(20, 1), Bound::Of(CompOp::Lt, 2)),
        (0, CompOp::Le, exact(20, 1), Bound::Of(CompOp::Le, 2)),
        // -1.5: floor -2, ceiling -1, toward negative infinity.
        (0, CompOp::Lt, exact(-15, 1), Bound::Of(CompOp::Lt, -1)),
        (0, CompOp::Le, exact(-15, 1), Bound::Of(CompOp::Le, -2)),
        // Fewer digits than the scale: scaled up.
        (2, CompOp::Eq, exact(3, 0), Bound::Of(CompOp::Eq, 300)),
        // Past an i64: by its sign.
        (0, CompOp::Lt, exact(100_000_000_000_000_000_000, 0), Bound::All),
        (0, CompOp::Gt, exact(100_000_000_000_000_000_000, 0), Bound::None),
        (0, CompOp::Lt, exact(-100_000_000_000_000_000_000, 0), Bound::None),
        (0, CompOp::Ge, exact(-100_000_000_000_000_000_000, 0), Bound::All),
        (0, CompOp::Eq, exact(-100_000_000_000_000_000_000, 0), Bound::None),
        (0, CompOp::Ne, exact(100_000_000_000_000_000_000, 0), Bound::All),
    ];
    for (scale, op, constant, bound) in cases {
        assert_eq!(rescaled_bound(scale, op, &constant), Some(bound), "{op:?} {constant} at scale {scale}");
    }
    assert_eq!(rescaled_bound(0, CompOp::Eq, &Value::Approx(1.5)), None, "not exact");
    assert_eq!(rescaled_bound(30, CompOp::Eq, &exact(i128::MAX, 0)), None, "too large to scale");
}

/// Verifies: [DESIGN] RULES.md D354 [X3.135-1986 §5.11 GR4] of every unit
/// near them, the bound a constant folds to keeps what comparing the
/// unit's exact number with the constant keeps.
#[test]
fn folded_bounds_keep_what_exact_comparison_keeps() {
    for scale in [0_u32, 1, 2] {
        for (units, constant_scale) in (-40_i128..=40).flat_map(|u| [0_u32, 1, 2, 3].map(|s| (u, s))) {
            folds_alike(scale, &exact(units, constant_scale));
        }
    }
}

/// Checks that of every operator `constant` folds at `scale` to a bound
/// keeping each unit near it as comparing its exact number does.
fn folds_alike(scale: u32, constant: &Value) {
    for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge] {
        let bound = rescaled_bound(scale, op, constant).unwrap();
        for u in -50_i64..=50 {
            let exact_truth = holds(op, crate::value::compare(&exact(i128::from(u), scale), constant));
            assert_eq!(by_bound(bound, u), exact_truth, "{u} at scale {scale} {op:?} {constant}");
        }
    }
}
