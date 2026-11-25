//! Column-only row projections consume their input values. Only repeated
//! output columns need a copy; an identity projection passes its row
//! through. Expression projections retain their ordinary evaluator (D31).
//! A projection of columns each named once, in the order of the row, moves
//! them to the front of the row it is given and cuts the rest off, so no
//! row is allocated (RULES.md D787).

use std::collections::HashSet;

use crate::error::Result;
use crate::logical::Expr;
use crate::storage::{Row, sized_row};
use crate::value::Value;

#[derive(Clone)]
pub(super) struct Columns {
    taken: Vec<(usize, bool)>,
    identity: bool,
    /// Whether each column is named once, in ascending order, so the row
    /// is taken in place.
    ascending: bool,
}

impl Columns {
    pub(super) fn of(exprs: &[Expr]) -> Option<Self> {
        let mut seen = HashSet::new();
        let mut columns = Vec::with_capacity(exprs.len());
        for expr in exprs.iter().rev() {
            let Expr::Column(column) = expr else { return None };
            columns.push((*column, !seen.insert(*column)));
        }
        columns.reverse();
        let identity = columns.iter().enumerate().all(|(at, &(column, _))| at == column);
        // Strictly ascending, so none is named twice, none copied.
        let ascending = columns.windows(2).all(|w| w.first().map(|c| c.0) < w.get(1).map(|c| c.0));
        Some(Self { taken: columns, identity, ascending })
    }

    pub(super) fn take(&self, mut row: Row) -> Result<Row> {
        if self.identity && row.len() == self.taken.len() {
            return Ok(row);
        }
        let width = row.len();
        if self.ascending {
            return in_place(&self.taken, row);
        }
        let taken = self.taken.iter().map(|&(at, copy)| {
            let value = row.get_mut(at).ok_or_else(|| super::broken(&format!("column {at} is not in a row of {width} columns")))?;
            Ok(if copy { value.clone() } else { std::mem::replace(value, Value::Null) })
        });
        sized_row(self.taken.len(), taken)
    }

    /// Select directly from a retained pair. Geometry refinement can
    /// borrow both rows and copy only the values the result needs.
    pub(super) fn pair(&self, left: &Row, right: &Row) -> Result<Row> {
        self.taken
            .iter()
            .map(|&(at, _)| {
                let value = if at < left.len() { left.get(at) } else { right.get(at.saturating_sub(left.len())) };
                value.cloned().ok_or_else(|| {
                    super::broken(&format!("column {at} is not in a pair of {} columns", left.len().saturating_add(right.len())))
                })
            })
            .collect()
    }
}

/// `row` projected on `columns`, each named once and in ascending order:
/// the column `columns` names `i`-th is swapped to place `i`, no later one
/// being at a place before its own, and the rest cut off (RULES.md D787).
fn in_place(columns: &[(usize, bool)], mut row: Row) -> Result<Row> {
    if let Some(&(last, _)) = columns.last()
        && last >= row.len()
    {
        return Err(super::broken(&format!("column {last} is not in a row of {} columns", row.len())));
    }
    for (place, &(at, _)) in columns.iter().enumerate() {
        // `at` is never before `place`, so its value heads the half that
        // begins there; where the two are one place, nothing moves.
        if let Some((front, back)) = row.split_at_mut_checked(at)
            && let (Some(to), Some(from)) = (front.get_mut(place), back.first_mut())
        {
            std::mem::swap(to, from);
        }
    }
    row.truncate(columns.len());
    Ok(row)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::error::Code;

    /// Verifies: [DESIGN] column projections preserve order, multiplicity
    /// and NULLs, including empty and identity projections. The last use
    /// consumes shared storage; missing columns still report SQLCODE -910.
    #[test]
    fn column_projections_move_last_uses_and_clone_only_duplicates() {
        for columns in [vec![], vec![0, 1, 2], vec![2, 0], vec![1, 2, 1, 0, 2]] {
            let expressions: Vec<_> = columns.iter().copied().map(Expr::Column).collect();
            let projection = Columns::of(&expressions).unwrap();
            let shared: Arc<[Value]> = Arc::from([Value::Char("payload".into())]);
            let row = vec![Value::Null, Value::Char("text".into()), Value::Row(Arc::clone(&shared))];
            let expected: Vec<_> = columns.iter().map(|&at| row[at].to_string()).collect();
            let result = projection.take(row).unwrap();
            assert_eq!(result.iter().map(ToString::to_string).collect::<Vec<_>>(), expected);
            assert_eq!(Arc::strong_count(&shared), columns.iter().filter(|&&at| at == 2).count().saturating_add(1));
        }
        for columns in [vec![0], vec![1, 0], vec![0, 0]] {
            let expressions: Vec<_> = columns.into_iter().map(Expr::Column).collect();
            assert_eq!(Columns::of(&expressions).unwrap().take(vec![]).unwrap_err().code, Code::Api);
        }
        assert!(Columns::of(&[Expr::Const(Value::Null)]).is_none());
        let identity = Columns::of(&[Expr::Column(0)]).unwrap();
        assert_eq!(identity.take(vec![Value::Null, Value::Null]).unwrap().len(), 1);
    }

    /// Verifies: [DESIGN] RULES.md D787: a projection of columns each
    /// named once in ascending order keeps the row it is given, the
    /// columns moved to its front, the rest cut off; one named out of order
    /// or twice makes a row of its own. A column past the row is SQLCODE
    /// -910 either way.
    ///
    /// Decision: each pair of neighbours ascending. MC/DC: all ascending
    /// (kept); a pair descending; a pair equal (made).
    #[test]
    fn ascending_projections_keep_their_row() {
        let row = || vec![Value::Char("a".into()), Value::Null, Value::Char("c".into()), Value::Char("d".into())];
        for (columns, kept) in [(vec![0, 2, 3], true), (vec![2], true), (vec![3, 2], false), (vec![2, 2], false), (vec![], true)] {
            let expressions: Vec<_> = columns.iter().copied().map(Expr::Column).collect();
            let given = row();
            let (at, expected) = (given.as_ptr(), columns.iter().map(|&c| row()[c].to_string()).collect::<Vec<_>>());
            let taken = Columns::of(&expressions).unwrap().take(given).unwrap();
            assert_eq!(taken.iter().map(ToString::to_string).collect::<Vec<_>>(), expected);
            assert_eq!(taken.as_ptr() == at, kept, "{columns:?}");
        }
        assert_eq!(Columns::of(&[Expr::Column(1), Expr::Column(4)]).unwrap().take(row()).unwrap_err().code, Code::Api);
    }

    /// Verifies: [DESIGN] D31: projecting a borrowed pair gives exactly
    /// the concatenated row's selected columns, including repeated,
    /// reordered and NULL values. Either side can be empty; missing
    /// columns report SQLCODE -910 instead of selecting another value.
    #[test]
    fn pair_projections_preserve_both_offsets_and_duplicates() {
        let values = vec![Value::Null, Value::Char("left".into()), Value::Char("right".into()), Value::Char("last".into())];
        for split in 0..=values.len() {
            let (left, right) = (values[..split].to_vec(), values[split..].to_vec());
            for columns in [vec![], vec![0, 1, 2, 3], vec![3, 0, 2, 1, 3], vec![1, 1]] {
                let exprs: Vec<_> = columns.into_iter().map(Expr::Column).collect();
                let projection = Columns::of(&exprs).unwrap();
                let actual = projection.pair(&left, &right).unwrap();
                let expected = projection.take(values.clone()).unwrap();
                assert_eq!(
                    actual.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    expected.iter().map(ToString::to_string).collect::<Vec<_>>()
                );
            }
            assert_eq!(Columns::of(&[Expr::Column(4)]).unwrap().pair(&left, &right).unwrap_err().code, Code::Api);
        }
    }
}
