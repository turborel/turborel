//! [ISO/IEC 9075-2:2023 §15.8 GR9, §15.14 GR12] The parts of the rows a
//! searched DELETE or UPDATE FOR PORTION OF a period changed that lie
//! outside the portion, kept as rows of their own: each a copy of the old
//! row, its period ending where the portion begins or beginning where it
//! ends, its identity column and generated columns given again as DEFAULT
//! gives them \[GR9b, GR12b\]. They are inserted once every row is
//! changed, as an INSERT inserts rows, its triggers executed and the
//! constraints checked as the statement ends (NOTE 707, NOTE 714).

use super::Written;
use crate::db::generated::Generating;
use crate::db::integrity::Effects;
use crate::db::{State, StatementContext};
use crate::error::Result;
use crate::exec::{HeldRows, Subqueries, eval};
use crate::logical::filled_expr;
use crate::storage::Row;

/// The rows kept of a statement's parts outside the portion.
pub(in crate::db) struct Leftovers {
    /// What gives a row kept its identity and generated columns, of a
    /// statement FOR PORTION OF; none of any other.
    generating: Option<Generating>,
    /// The rows kept.
    rows: HeldRows<Row>,
}

impl Leftovers {
    /// Of the statement `w`, none yet.
    ///
    /// # Errors
    ///
    /// As `Generating::of`.
    pub(in crate::db) fn of(st: &State, w: &Written) -> Result<Leftovers> {
        let generating = w.bound.portion.as_ref().map(|_| Generating::of(st, &w.bound.desc, true)).transpose()?;
        Ok(Leftovers { generating, rows: HeldRows::new(st.list_space()) })
    }

    /// [GR9c, GR9d; GR12c, GR12d] Keeps the parts of `old`, a row `w`
    /// changed, before and after the portion, if it has them.
    ///
    /// # Errors
    ///
    /// A bound fails, or a row kept is given no identity or generated value.
    pub(in crate::db) fn keep(&mut self, st: &State, w: &Written, old: &Row, subqueries: &dyn Subqueries) -> Result<()> {
        let (Some(pieces), Some(generating)) = (&w.bound.portion, &self.generating) else { return Ok(()) };
        let [start, end] = pieces.columns;
        let [before, after] = &pieces.bounds;
        // The part before has the portion's start as its end, the part
        // after the portion's end as its start.
        for (bound, column) in [(before, end), (after, start)] {
            let value = match &w.values {
                Some(values) => eval(&filled_expr(bound.clone(), values)?, old, subqueries)?,
                None => eval(bound, old, subqueries)?,
            };
            if value.is_null() {
                continue;
            }
            let mut row = old.clone();
            if let Some(slot) = row.get_mut(column) {
                *slot = value;
            }
            generating.give(st, &mut row, subqueries)?;
            self.rows.push(row)?;
        }
        Ok(())
    }

    /// Inserts the rows kept into the object table of `w`, its BEFORE
    /// triggers first, each noted in `effects`.
    ///
    /// # Errors
    ///
    /// As INSERT fails.
    pub(in crate::db) fn insert(self, cx: StatementContext<'_>, w: &Written, effects: &mut Effects<'_>) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let table = &w.bound.table;
        let mut rows = effects.before_insert(cx, table, self.rows)?;
        rows.visit(&mut |row| cx.st.insert_routed(cx.tx, table, row).and_then(|(leaf, id)| effects.inserted(&leaf, id, row)))
    }
}
