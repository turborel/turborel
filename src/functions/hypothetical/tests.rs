//! Unit tests of the hypothetical set functions' accumulator (RULES.md
//! R-TEST-1, R-TEST-8): each function against its window function's value
//! for the hypothetical row, found by sorting the group with it and
//! reading the row's peer group, over groups of ties and nulls in every
//! direction and null ordering; the group of no rows.

use super::*;

/// A key of an exact number, or null.
fn key(n: Option<i128>) -> Value {
    n.map_or(Value::Null, |n| Value::Exact(Decimal::new(n, 0)))
}

/// The accumulator of `hypothesis` ordered by one key in `order`.
fn ranked(hypothesis: Hypothesis, order: SortOrder) -> Box<dyn Accumulator> {
    let f = Hypothetical { hypothesis, result: DataType::Boolean, orders: Arc::from([order]) };
    f.accumulator(DataType::Boolean)
}

/// The text of `hypothesis` of `h` among `rows`, fed each row, or each
/// distinct row once of DENSE_RANK, as grouping feeds it.
fn fed(hypothesis: Hypothesis, order: SortOrder, (h, rows): (Option<i128>, &[Option<i128>])) -> String {
    let mut acc = ranked(hypothesis, order);
    let mut seen = Vec::new();
    for r in rows {
        if hypothesis == Hypothesis::DenseRank && seen.contains(r) {
            continue;
        }
        seen.push(*r);
        acc.update(&Value::Row(Arc::from([key(h), key(*r)]))).unwrap();
    }
    acc.finish().unwrap().to_string()
}

/// The text of the window function `hypothesis` for the hypothetical row
/// `h` of a partition of `rows` and it, ordered so \[GR9\]: the row's peer
/// group found in the sorted partition, its first and last places, and
/// the peer groups before it.
fn windowed(hypothesis: Hypothesis, order: SortOrder, (h, rows): (Option<i128>, &[Option<i128>])) -> String {
    let mut partition: Vec<(Value, bool)> = rows.iter().map(|r| (key(*r), false)).collect();
    partition.push((key(h), true));
    partition.sort_by(|a, b| order.cmp(&a.0, &b.0));
    let at = partition.iter().position(|(_, marked)| *marked).unwrap();
    let peer = |i: &usize| order.cmp(&partition[*i].0, &partition[at].0).is_eq();
    let first = (0..partition.len()).find(peer).unwrap();
    let last = (0..partition.len()).rev().find(peer).unwrap();
    let groups = (1..=first).filter(|&i| order.cmp(&partition[i.checked_sub(1).unwrap()].0, &partition[i].0).is_ne()).count();
    let n = partition.len();
    let whole = |x: usize| Value::Exact(Decimal::new(i128::try_from(x).unwrap(), 0)).to_string();
    let real = |x: usize| Decimal::new(i128::try_from(x).unwrap(), 0).to_f64();
    match hypothesis {
        Hypothesis::Rank => whole(first.checked_add(1).unwrap()),
        Hypothesis::DenseRank => whole(groups.checked_add(1).unwrap()),
        Hypothesis::PercentRank => {
            let others = n.checked_sub(1).unwrap();
            Value::Approx(if others == 0 { 0.0 } else { real(first) / real(others) }).to_string()
        }
        Hypothesis::CumeDist => Value::Approx(real(last.checked_add(1).unwrap()) / real(n)).to_string(),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9, §9.23 SR5e to
/// SR5h] each hypothetical set function is its window function's value
/// for the hypothetical row among its group's rows: of a value among them
/// or between, before or after all, null or not, among ties and nulls,
/// ascending and descending, nulls first and last.
///
/// Decision: of each row, before the hypothetical row, its peer, or
/// after. Each group holds rows of each, and each changes the outcome
/// alone, as the brute force shows.
#[test]
fn each_function_is_its_window_functions_value_10_9() {
    let groups: [&[Option<i128>]; 6] = [
        &[Some(3), Some(1), Some(2)],
        &[Some(2), Some(2), None, Some(5), Some(1), Some(2)],
        &[None, None],
        &[Some(4)],
        &[Some(1), None, Some(1), Some(7), None, Some(3), Some(3)],
        &[Some(5), Some(5), Some(5)],
    ];
    let hypotheticals = [None, Some(0), Some(1), Some(2), Some(3), Some(5), Some(9)];
    for order in [SortOrder::of(false, None), SortOrder::of(true, None), SortOrder::of(false, Some(true)), SortOrder::of(true, Some(false))]
    {
        for (_, hypothesis) in RANK_FUNCTION_TYPES {
            for rows in groups {
                for h in hypotheticals {
                    let case = (h, rows);
                    assert_eq!(fed(hypothesis, order, case), windowed(hypothesis, order, case), "{hypothesis:?} {order:?} {case:?}");
                }
            }
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9] of a group of no
/// rows the hypothetical row is alone in its partition: RANK and
/// DENSE_RANK 1, PERCENT_RANK 0 and CUME_DIST 1; and a row not of the
/// values and keys it was bound with is a failure, not a panic.
#[test]
fn no_rows_rank_first_10_9() {
    let order = SortOrder::of(false, None);
    let shown = |h| ranked(h, order).finish().unwrap().to_string();
    assert_eq!(shown(Hypothesis::Rank), "1");
    assert_eq!(shown(Hypothesis::DenseRank), "1");
    assert_eq!(shown(Hypothesis::PercentRank), Value::Approx(0.0).to_string());
    assert_eq!(shown(Hypothesis::CumeDist), Value::Approx(1.0).to_string());
    let mut acc = ranked(Hypothesis::Rank, order);
    assert_eq!(acc.update(&key(Some(1))).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(acc.update(&Value::Row(Arc::from([key(Some(1))]))).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9, §10.10 GR2] of
/// several keys a row is placed by the first it differs in, each in its
/// own direction: (1, 9) before (2, 0) ascending, (2, 5) before (2, 3)
/// descending in the second, peers only if alike in both.
#[test]
fn several_keys_place_by_the_first_unlike_10_9() {
    let orders: Arc<[SortOrder]> = Arc::from([SortOrder::of(false, None), SortOrder::of(true, None)]);
    let f = Hypothetical { hypothesis: Hypothesis::CumeDist, result: DataType::Boolean, orders };
    let mut acc = f.accumulator(DataType::Boolean);
    let row = |a: i128, b: i128| Value::Row(Arc::from([key(Some(2)), key(Some(4)), key(Some(a)), key(Some(b))]));
    for (a, b) in [(1, 9), (2, 5), (2, 4), (2, 3), (3, 0)] {
        acc.update(&row(a, b)).unwrap();
    }
    // Before: (1, 9) and (2, 5); a peer: (2, 4); so 4 of 6 not after it.
    assert_eq!(acc.finish().unwrap().to_string(), Value::Approx(4.0 / 6.0).to_string());
}
