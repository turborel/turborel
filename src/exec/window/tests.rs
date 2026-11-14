use super::*;
use crate::value::Decimal;

/// The current row `row` of peers from `from` up to `to`, of the peer
/// group `group`.
fn at(row: usize, group: u64, (from, to): (usize, usize)) -> Current {
    Current { row, group, peers: (from, to), key: Value::Null }
}

fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5d, SR5e, SR5f] of
/// the fourth of five rows, its peers the third and fourth, of the second
/// peer group after one of two rows: ROW_NUMBER 4, RANK 3, DENSE_RANK 2,
/// each exact of scale 0 (IV166).
#[test]
fn ranks_count_rows_and_peer_groups_9_23() {
    let current = at(3, 1, (2, 4));
    assert_eq!(format!("{:?}", rank(&WindowKind::RowNumber, &current, 5).unwrap()), format!("{:?}", exact(4)));
    assert_eq!(format!("{:?}", rank(&WindowKind::Rank, &current, 5).unwrap()), format!("{:?}", exact(3)));
    assert_eq!(format!("{:?}", rank(&WindowKind::DenseRank, &current, 5).unwrap()), format!("{:?}", exact(2)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5g, SR5h]
/// PERCENT_RANK is (RANK - 1) / (rows - 1), 0 of a partition of one row,
/// where the divisor is 0; CUME_DIST the rows up to the last peer over the
/// rows; both approximate (IV082, IV083).
///
/// Decision: the partition has one row or more; each changes PERCENT_RANK
/// alone.
#[test]
fn relative_ranks_are_fractions_of_the_partition_9_23() {
    let real = |kind: &WindowKind, current: &Current, len: usize| {
        let found = rank(kind, current, len).unwrap();
        let Value::Approx(x) = found else { panic!("not approximate: {found:?}") };
        x
    };
    let third = at(2, 1, (1, 4));
    assert!((real(&WindowKind::PercentRank, &third, 5) - 0.25).abs() < f64::EPSILON);
    assert!((real(&WindowKind::CumeDist, &third, 5) - 0.8).abs() < f64::EPSILON);
    let alone = at(0, 0, (0, 1));
    assert!(real(&WindowKind::PercentRank, &alone, 1).abs() < f64::EPSILON);
    assert!((real(&WindowKind::CumeDist, &alone, 1) - 1.0).abs() < f64::EPSILON);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR4] the peers of each
/// row are the rows of its peer group, found as the rows are read in
/// order: a group of one row, of three, and the last.
#[test]
fn peers_are_found_row_after_row_7_15() {
    let mut keys = HeldRows::new(ListSpace::default());
    for group in [0_u64, 1, 1, 1, 2] {
        keys.push(Placed { group, values: vec![Value::Null] }).unwrap();
    }
    let mut part = Partition { keys, len: 5, nulls: (5, 5) };
    let mut peers = Peers::default();
    let found: Vec<(u64, (usize, usize))> =
        (0..5).map(|row| current(row, &mut part, &mut peers).map(|c| (c.group, c.peers)).unwrap()).collect();
    assert_eq!(found, [(0, (0, 1)), (1, (1, 4)), (1, (1, 4)), (1, (1, 4)), (2, (4, 5))]);
}
