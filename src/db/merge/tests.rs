//! Unit tests of a MERGE's decisions that its statements reach only in
//! part: each target row's subject clause and the cardinality rule over
//! its pairs, and each unmatched source row's WHEN NOT MATCHED clause.

use super::decide::{Grouping, Subject, inserting};
use crate::binder::{BoundAction, BoundWhen};
use crate::error::{Code, SqlState};
use crate::storage::Row;
use crate::value::Value;

/// A joined row telling its pairs apart by `n`.
fn row(n: u8) -> Row {
    vec![Value::Char(n.to_string().into())]
}

/// Of each subject row, its clause, its row id and its row, shown.
type Seen = Vec<(usize, u64, String)>;

/// The subject rows `pairs` decide, each pair a target row's id, the
/// first clause true of it and its row, in the order given.
fn subjects(pairs: &[(u64, Option<usize>, u8)]) -> crate::error::Result<Seen> {
    let mut grouping = Grouping::default();
    let mut out: Vec<Subject> = Vec::new();
    for &(id, first, n) in pairs {
        out.extend(grouping.pair((0, id), first, row(n))?);
    }
    out.extend(grouping.finish()?);
    Ok(out.into_iter().map(|Subject { clause, id, joined }| (clause, id, format!("{joined:?}"))).collect())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 A] a target row's
/// subject clause is the first clause true of any of its pairs, whichever
/// pair comes first, and its matching row is the pair it is true of.
#[test]
fn the_subject_clause_is_the_first_any_pair_makes_true() {
    let found = subjects(&[(7, Some(2), 1), (7, Some(0), 2), (7, None, 3)]).unwrap();
    assert_eq!(found, vec![(0, 7, format!("{:?}", row(2)))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 A] a target row no
/// clause is true of for any pair is no subject row; the rows around it
/// still are.
#[test]
fn a_target_row_no_clause_applies_to_is_left() {
    let found = subjects(&[(1, Some(1), 1), (2, None, 2), (2, None, 3), (3, Some(0), 4)]).unwrap();
    assert_eq!(found, vec![(1, 1, format!("{:?}", row(1))), (0, 3, format!("{:?}", row(4)))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B II] two pairs of one
/// target row its subject clause is true of are a cardinality violation,
/// SQLSTATE 21000, duplicates of one source row too.
#[test]
fn two_matching_rows_of_the_subject_clause_are_refused() {
    let refused = subjects(&[(4, Some(1), 1), (4, Some(1), 1)]).unwrap_err();
    assert_eq!(refused.code, Code::Cardinality);
    assert_eq!(refused.sqlstate(), SqlState::CARDINALITY_VIOLATION);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B I] M counts only the
/// pairs the subject clause is true of. MC/DC of the choice: a later pair
/// of an earlier clause restarts the count (clause 0 after two pairs of
/// clause 1 is the subject, M = 1); a pair of a later clause is not
/// counted (clause 2 after clause 0, M = 1); one of the same clause is
/// (as `two_matching_rows_of_the_subject_clause_are_refused`).
#[test]
fn only_pairs_of_the_subject_clause_are_counted() {
    let found = subjects(&[(5, Some(1), 1), (5, Some(1), 2), (5, Some(0), 3), (5, Some(2), 4)]).unwrap();
    assert_eq!(found, vec![(0, 5, format!("{:?}", row(3)))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B II] the violation is
/// raised at the end of the target row's pairs, the last row too.
#[test]
fn the_last_target_row_is_checked_too() {
    let refused = subjects(&[(1, Some(0), 1), (2, None, 2), (2, Some(0), 3), (2, Some(0), 4)]).unwrap_err();
    assert_eq!(refused.code, Code::Cardinality);
}

/// Verifies: [DESIGN] a pair of a target row need be tried only against
/// the clauses up to the one its pairs chose so far; a new target row
/// against all.
#[test]
fn a_pair_is_tried_only_up_to_the_clause_chosen() {
    let mut grouping = Grouping::default();
    assert_eq!(grouping.limit((0, 9)), None);
    assert!(grouping.pair((0, 9), None, row(1)).unwrap().is_none());
    assert_eq!(grouping.limit((0, 9)), None);
    assert!(grouping.pair((0, 9), Some(3), row(2)).unwrap().is_none());
    assert_eq!(grouping.limit((0, 9)), Some(3));
    assert_eq!(grouping.limit((0, 10)), None);
}

/// A WHEN NOT MATCHED clause of flag `flag`.
fn not_matched(flag: Option<usize>) -> BoundWhen {
    BoundWhen::NotMatched { flag, object: Vec::new(), values: Vec::new() }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a ii 5] an unmatched source
/// row is inserted by the first WHEN NOT MATCHED clause that applies: one
/// of no condition, or one whose flag is true, WHEN MATCHED clauses passed
/// over. MC/DC of a clause applying: no flag applies; a flag true applies;
/// a flag false or unknown does not.
#[test]
fn the_first_not_matched_clause_that_applies_inserts() {
    let clauses = vec![
        BoundWhen::Matched { condition: None, action: BoundAction::Delete },
        not_matched(Some(0)),
        not_matched(Some(1)),
        not_matched(None),
    ];
    let flags = |a: Value, b: Value| vec![a, b];
    assert_eq!(inserting(&clauses, &flags(Value::Bool(true), Value::Bool(true))), Some(1));
    assert_eq!(inserting(&clauses, &flags(Value::Bool(false), Value::Bool(true))), Some(2));
    assert_eq!(inserting(&clauses, &flags(Value::Null, Value::Bool(false))), Some(3));
    assert_eq!(inserting(&clauses[..3], &flags(Value::Bool(false), Value::Null)), None);
}
