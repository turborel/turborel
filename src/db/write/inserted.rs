//! \[DESIGN\] The query of a kept INSERT read by its physical plan kept or
//! learned, as a prepared query's is (RULES.md D793): not planned again at
//! each execution once its plans agree.

use std::sync::Arc;

use super::Target;
use crate::db::Session;
use crate::db::StatementContext;
use crate::db::Stream;
use crate::db::prepared::Keeping;
use crate::db::routines::Hosted;
use crate::error::Result;
use crate::exec::{Db, HeldRows};
use crate::params::Params;
use crate::storage::Row;

/// The candidate rows of `target`'s query, of an INSERT kept by `k` with
/// `params`, by its physical plan kept or learning (`Keeping::physical`);
/// none if it is planned with its values, or its statement is analyzed,
/// whose plan EXPLAIN ANALYZE counts.
///
/// # Errors
///
/// A lock is not granted; as `Keeping::physical` and `Target::candidate`.
pub(super) fn kept_rows(
    (session, cx): (&Session, StatementContext<'_>),
    target: &Target,
    kept: Option<(&Keeping<'_>, &Params)>,
) -> Result<Option<HeldRows<Row>>> {
    let Some((k, params)) = kept.filter(|_| !cx.tx.stop.changed().taken()) else { return Ok(None) };
    let Some((_, run, reads, slots)) = k.physical(params)? else { return Ok(None) };
    let db = slots.as_ref().map_or_else(|| Db::of(cx.st), |slots| Db::slotted(cx.st, Arc::clone(slots)));
    let hosted = Hosted::in_db(Some(session), cx, &db);
    let mut stream = Stream::run(cx.st, cx.tx, run, (reads, slots))?;
    // Held up to MEMORY.WORK_BYTES, the rest spilled (RULES.md D38).
    let mut rows = HeldRows::new(cx.st.list_space());
    while let Some(values) = stream.next()? {
        rows.push(target.candidate(cx.st, values, &hosted)?)?;
    }
    Ok(Some(rows))
}
