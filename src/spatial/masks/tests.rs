//! Boundary masks agree with independent closed rectangle comparisons.

use super::*;

fn linear(bounds: &[Option<Bounds>], query: Bounds) -> u64 {
    bounds.iter().enumerate().fold(
        0_u64,
        |mask, (at, bounds)| {
            if bounds.is_some_and(|bounds| bounds.intersects(query)) { mask | (1_u64 << at) } else { mask }
        },
    )
}

/// Verifies: [DESIGN] D82: mask lookup keeps exactly overlapping closed
/// bounds, including repeated endpoints, touching edges, degenerate boxes,
/// absent bounds, signed zero, subnormals and extreme finite coordinates.
/// Every word bit, empty input and the capacity limit are exercised.
#[test]
fn boundary_masks_match_closed_bounds_at_every_bit_and_limit() {
    let tiny = f64::from_bits(1);
    let coordinates = [-f64::MAX, -1.0_f64, -tiny, -0.0_f64, 0.0_f64, tiny, 1.0_f64, f64::MAX];
    let mut bounds = Vec::new();
    for &low in &coordinates {
        for &high in &coordinates {
            bounds.push(Some(Bounds::new([low.min(high), -1.0_f64], [low.max(high), 1.0_f64]).unwrap()));
        }
    }
    for count in 0..=ROWS {
        for missing in [false, true] {
            let mut subset = bounds[..count].to_vec();
            if missing {
                for bound in subset.iter_mut().step_by(3) {
                    *bound = None;
                }
            }
            let masks = Masks::new(&subset).unwrap();
            for &x in &coordinates {
                for y in [-2.0_f64, -1.0_f64, 0.0_f64, 1.0_f64, 2.0_f64] {
                    let query = Bounds::point([x, y]).unwrap();
                    assert_eq!(masks.overlapping(query), linear(&subset, query));
                }
            }
            for query in bounds.iter().flatten() {
                assert_eq!(masks.overlapping(*query), linear(&subset, *query));
            }
        }
    }
    assert!(Masks::new(&vec![None; ROWS.saturating_add(1)]).is_none());
}
