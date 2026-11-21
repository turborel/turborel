//! \[EXTENSION\] The batches of an operator EXPLAIN ANALYZE counts (RULES.md
//! D376, D378): its input's, each batch pulled timed and its rows kept
//! counted at once, so counting costs a batch what it costs a row on the
//! row path (`exec/counted.rs`), and the batches noted (RULES.md D621).

use std::sync::Arc;

use super::late::Deferred;
use super::stream::{self, Batches, Kept};
use crate::db::State;
use crate::error::Result;
use crate::exec::broken;
use crate::exec::counted::Tally;
use crate::physical::{Counted, Plan};
use crate::storage::Sel;
use crate::txn::TxId;

/// The batches of `plan`, a counted operator, counted, its scans' columns
/// `deferred` leaves unread given late as its input's are.
///
/// # Errors
///
/// As its input's batches; SQLCODE -910 if it is not counted.
pub(super) fn batches(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::Counted(c) = plan else { return Err(broken("a counted operator is not one")) };
    Ok(through(*c, db, |input| Ok((stream::batches_with(input, (db, tx), deferred)?, ())))?.0)
}

/// The batches `run` gives of `c`'s operator, with what else it gives,
/// counted as `c`'s: its opening timed, then each batch pulled.
///
/// # Errors
///
/// As `run`.
pub(super) fn through<T>(c: Counted, db: &State, run: impl FnOnce(Plan) -> Result<(Batches, T)>) -> Result<(Batches, T)> {
    let Counted { count, input } = c;
    let mut tally = Tally::opened(count, db);
    let (inner, more) = tally.timed(|| run(input))?;
    Ok((Box::new(CountedBatches { inner, tally }), more))
}

/// The batches of a counted operator, each pull timed and its rows kept
/// counted.
struct CountedBatches {
    inner: Batches,
    tally: Tally,
}

impl Iterator for CountedBatches {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Result<Kept>> {
        let inner = &mut self.inner;
        let pulled = self.tally.timed(|| inner.next());
        if let Some(Ok((batch, kept))) = &pulled {
            let rows = kept.as_ref().map_or(batch.rows, Sel::len);
            self.tally.batch(u64::try_from(rows).unwrap_or(u64::MAX));
        }
        pulled
    }
}
