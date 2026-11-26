//! \[DESIGN\] A projection of columns moved (RULES.md D31, D1167): the
//! place of each column it gives among those a batch holds, and the
//! batch of those vectors, each moved, a null value's a vector of nulls.

use crate::logical::Expr;
use crate::storage::{Batch, Column};

/// The place among `columns` of each of `exprs`, if each is a column they
/// hold or the null value, which is at none (RULES.md D1167).
pub(super) fn picked(exprs: &[Expr], columns: &[usize]) -> Option<Vec<Option<usize>>> {
    exprs
        .iter()
        .map(|e| {
            if let Expr::Column(c) = e {
                columns.iter().position(|held| held == c).map(Some)
            } else if let Expr::Const(v) = e {
                v.is_null().then_some(None)
            } else {
                None
            }
        })
        .collect()
}

/// `batch` with the vectors at `picks`, in order: each moved, or copied if
/// a later pick takes it too; of a pick of none, a vector of nulls.
pub(super) fn pick(batch: Batch, picks: &[Option<usize>]) -> Batch {
    let mut held: Vec<Option<Column>> = batch.columns.into_iter().map(Some).collect();
    let columns = picks
        .iter()
        .enumerate()
        .map(|(n, &pick)| {
            let Some(at) = pick else { return Column::nulls(batch.rows) };
            let again = picks.get(n.saturating_add(1)..).is_some_and(|rest| rest.contains(&pick));
            let vector = if again { held.get(at).cloned().flatten() } else { held.get_mut(at).and_then(Option::take) };
            vector.unwrap_or_default()
        })
        .collect();
    Batch { columns, rows: batch.rows }
}
