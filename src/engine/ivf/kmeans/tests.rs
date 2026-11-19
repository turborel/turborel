//! Tests of k-means on samples that leave centres without points.

use std::sync::Arc;

use super::*;

/// Squared Euclidean distance, as IVF measures by it.
fn metric() -> Metric {
    Metric::new("EUCLIDEAN_SQUARED", Arc::new(|a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()))
}

/// Verifies: [DESIGN] vector design document §10.4: an empty sample
/// trains no centre; of a sample of one point repeated, k-means++ draws
/// every centre at that point, and the centres Lloyd's iterations leave
/// without points each take a point of the sample, so every list stays in
/// use; a sample of distinct points gives as many centres as asked.
#[test]
fn centres_left_without_points_take_the_farthest() {
    let m = metric();
    assert_eq!(train(&m, &[], (2, 3), 1).len(), 0);
    let same = [1.0_f32, 2.0].repeat(5);
    let centres = train(&m, &same, (2, 3), 1);
    assert_eq!(centres.len(), 6);
    assert_eq!(centres, [1.0_f32, 2.0].repeat(3));
    let spread: Vec<f32> = (0..20_u16).flat_map(|i| [f32::from(i % 4) * 10.0, f32::from(i)]).collect();
    let spread_centres = train(&m, &spread, (2, 4), 3);
    assert_eq!(spread_centres.len(), 8);
    assert!(spread_centres.iter().all(|x| x.is_finite()));
}

/// Verifies: [DESIGN] vector design document §10.4: a centre with no
/// point assigned takes the point farthest from its own centre, each such
/// point taken once; with every point taken, it keeps none.
#[test]
fn an_empty_centre_takes_the_farthest_point_once() {
    let m = metric();
    let sample = [0.0_f32, 0.0, 1.0, 0.0, 9.0, 0.0];
    let centres = [0.0_f32, 0.0, 100.0, 100.0, 200.0, 200.0];
    let moved = means(&sample, (&centres, 2), &[0, 0, 0], &m);
    assert_eq!(moved, [10.0 / 3.0, 0.0, 9.0, 0.0, 1.0, 0.0]);
    let mut taken = vec![true; 3];
    assert!(farthest(&sample, (&centres, 2), &[0, 0, 0], (&m, &mut taken)).is_none());
}
