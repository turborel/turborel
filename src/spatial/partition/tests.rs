//! Requirements for the R* split and subtree-choice heuristics.

use super::*;

fn square(x: f64, y: f64) -> Bounds {
    Bounds::new([x, y], [x + 1.0_f64, y + 1.0_f64]).unwrap()
}

/// Verifies: [DESIGN] R* splits choose the separated axis and minimize
/// combined area after overlap. Both permutations represent the same
/// separated clusters; neither insertion order nor axis names decide.
#[test]
fn separated_clusters_split_between_the_clusters() {
    for transpose in [false, true] {
        let bounds: Vec<_> = (0_i32..10_i32)
            .map(|i| {
                let x = f64::from(i.checked_div(2).unwrap() + (i % 2_i32) * 100_i32);
                if transpose { square(0.0_f64, x) } else { square(x, 0.0_f64) }
            })
            .collect();
        let split = split(&bounds).unwrap();
        assert_eq!(split.at, 5);
        let left: Vec<_> = split.order[..split.at].iter().map(|&i| bounds[i]).collect();
        let right: Vec<_> = split.order[split.at..].iter().map(|&i| bounds[i]).collect();
        let left = left.into_iter().reduce(Bounds::union).unwrap();
        let right = right.into_iter().reduce(Bounds::union).unwrap();
        assert!(!left.intersects(right));
    }
}

/// Verifies: [DESIGN] Every split preserves every entry once and the
/// minimum occupancy, including coincident points and extreme finite
/// coordinates; the normalized area calculations must stay finite.
#[test]
fn degenerate_and_extreme_boxes_keep_legal_partitions() {
    for count in 2_usize..100 {
        let bounds: Vec<_> = (0..count)
            .map(|i| {
                let x = if i % 2 == 0 { f64::MAX } else { -f64::MAX };
                Bounds::point([x, x]).unwrap()
            })
            .collect();
        let split = split(&bounds).unwrap();
        let minimum = (count * 2).checked_div(5).unwrap().max(1);
        assert!(split.at >= minimum);
        assert!(count - split.at >= minimum);
        let mut order = split.order;
        order.sort_unstable();
        assert_eq!(order, (0..count).collect::<Vec<_>>());
    }
    assert_eq!(split(&[]).err().unwrap().code, Code::Corrupt);
    assert_eq!(split(&[square(0.0_f64, 0.0_f64)]).err().unwrap().code, Code::Corrupt);
}

/// Verifies: [DESIGN] A subtree already enclosing the new rectangle
/// wins over one requiring enlargement; a smaller containing rectangle
/// breaks an enlargement tie. Empty branches are rejected as corruption.
#[test]
fn subtree_choice_minimizes_enlargement_then_area() {
    let large = Bounds::new([-10.0_f64, -10.0_f64], [10.0_f64, 10.0_f64]).unwrap();
    let small = square(0.0_f64, 0.0_f64);
    for above_leaves in [false, true] {
        assert_eq!(choose(&[large, small], Bounds::point([0.5_f64, 0.5_f64]).unwrap(), above_leaves).unwrap(), 1);
        assert_eq!(choose(&[small, large], square(5.0_f64, 5.0_f64), above_leaves).unwrap(), 1);
        assert_eq!(choose(&[], small, above_leaves).unwrap_err().code, Code::Corrupt);
        assert_eq!(choose(&[large], small, above_leaves).unwrap(), 0);
    }
}

/// Verifies: [DESIGN] Leaf-parent choice considers overlap with every
/// sibling even when there are more siblings than the shortlist limit.
/// Distant disjoint boxes never displace a containing candidate.
#[test]
fn overlap_shortlist_handles_wide_nodes() {
    let mut bounds: Vec<_> = (0_i32..100_i32).map(|i| square(f64::from(i * 10_i32), 0.0_f64)).collect();
    bounds.reverse();
    let added = Bounds::point([0.5_f64, 0.5_f64]).unwrap();
    assert_eq!(choose(&bounds, added, true).unwrap(), 99);
}

/// Verifies: [DESIGN] Above leaves, avoiding overlap takes precedence
/// over the area rule used higher in the tree. Extending the lower-left
/// box would overlap the long box; extending the upper-left box does not.
#[test]
fn overlap_enlargement_precedes_area_above_leaves() {
    let bounds = [
        Bounds::new([0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]).unwrap(),
        Bounds::new([0.0_f64, 4.0_f64], [2.0_f64, 6.0_f64]).unwrap(),
        Bounds::new([2.0_f64, 0.0_f64], [16.0_f64, 2.0_f64]).unwrap(),
    ];
    let added = Bounds::point([3.0_f64, 3.0_f64]).unwrap();
    assert_eq!(choose(&bounds, added, false).unwrap(), 0);
    assert_eq!(choose(&bounds, added, true).unwrap(), 1);
}
