//! [X3.135-1986 §8.7 GR3, GR4] The candidate rows of an INSERT's query.
//! \[DESIGN\] Those of a kept INSERT are read by its physical plan kept or
//! learned, as a prepared query's is (RULES.md D793): not planned again at
//! each execution once its plans agree. Its plan bound is lent by what
//! keeps it, and copied only where an execution fills and plans it
//! (RULES.md D902).

use std::sync::Arc;

use super::Target;
use crate::catalog::QualifiedName;
use crate::db::Session;
use crate::db::StatementContext;
use crate::db::Stream;
use crate::db::prepared::{Keeping, Run};
use crate::db::routines::{Hosted, hosted_rows};
use crate::db::{explain, plan};
use crate::error::Result;
use crate::exec::{Db, HeldRows, Lent, Slots, execute};
use crate::logical::{LogicalPlan, filled};
use crate::params::Params;
use crate::storage::Row;

/// [X3.135-1986 §8.7 GR3, GR4; ISO/IEC 9075:1992 §13.8 GR3, GR4] the
/// candidate rows of a `<query expression>`, bound as `logical`, one for
/// each row of its result, which is evaluated in full before any row is
/// inserted.
///
/// Of VALUES that invoke a function that possibly modifies SQL-data, each
/// row is evaluated in turn by the session (`routines::hosted_rows`).
pub(super) fn query_rows(
    (session, cx): (&Session, StatementContext<'_>),
    target: &Target,
    (logical, slots, kept): (&LogicalPlan, Option<Slots>, Option<(&Keeping<'_>, &Params)>),
) -> Result<HeldRows<Row>> {
    let StatementContext { st, tx } = cx;
    let db = slots.as_ref().map_or_else(|| Db::of(st), |slots| Db::slotted(st, Arc::clone(slots)));
    let hosted = Hosted::in_db(Some(session), cx, &db);
    // Held up to MEMORY.WORK_BYTES, the rest spilled (RULES.md D38).
    let mut rows = HeldRows::new(st.list_space());
    if let Some(found) = hosted_rows(logical, &hosted) {
        for values in found? {
            rows.push(target.candidate(st, values, &hosted)?)?;
        }
        return Ok(rows);
    }
    // A query other than a row of values, by its plan kept (RULES.md
    // D793); else planned with its values, as its paths follow them.
    if let Some(held) = kept_rows((session, cx), target, kept)? {
        return Ok(held);
    }
    // Lent by what keeps it, it is copied only here, to be planned.
    let logical = match &slots {
        Some(slots) => filled(logical.clone(), slots)?,
        None => logical.clone(),
    };
    let (optimized, _, physical) = plan(st, logical);
    for t in optimized.tables() {
        st.read_lock(tx, &t)?;
    }
    for values in execute(explain::analyzing(tx, st, (physical, Some(&optimized))), st, tx.id) {
        rows.push(target.candidate(st, values?, &hosted)?)?;
    }
    Ok(rows)
}

/// The candidate rows of `target`'s query, of an INSERT `kept`, its
/// keeping and its parameters, by its physical plan kept or learning
/// (`Keeping::physical`);
/// none if it is planned with its values, or its statement is analyzed,
/// whose plan EXPLAIN ANALYZE counts.
///
/// # Errors
///
/// A lock is not granted; as `Keeping::physical` and `Target::candidate`.
fn kept_rows(
    (session, cx): (&Session, StatementContext<'_>),
    target: &Target,
    kept: Option<(&Keeping<'_>, &Params)>,
) -> Result<Option<HeldRows<Row>>> {
    let Some((k, params)) = kept.filter(|_| !cx.tx.stop.changed().taken()) else { return Ok(None) };
    let Some((_, run, reads, slots)) = k.physical(params)? else { return Ok(None) };
    let db = slots.as_ref().map_or_else(|| Db::of(cx.st), |slots| Db::slotted(cx.st, Arc::clone(slots)));
    let hosted = Hosted::in_db(Some(session), cx, &db);
    // Held up to MEMORY.WORK_BYTES, the rest spilled (RULES.md D38).
    let mut rows = HeldRows::new(cx.st.list_space());
    let Some(found) = lent_rows(cx, &run, (&reads, &db))? else {
        let mut stream = Stream::run(cx.st, cx.tx, run, (reads, slots))?;
        while let Some(values) = stream.next()? {
            rows.push(target.candidate(cx.st, values, &hosted)?)?;
        }
        return Ok(Some(rows));
    };
    for values in found {
        rows.push(target.candidate(cx.st, values?, &hosted)?)?;
    }
    Ok(Some(rows))
}

/// \[DESIGN\] RULES.md D903: the rows of `run`, a plan kept that reads
/// `reads`, read where it is kept, with no copy of it made, if it is
/// read so (`exec::Lent`): every row found before a candidate row is
/// made of any, as a plan that can raise has its rows read as it opens
/// (`Stream::open`). Its tables are locked and its subqueries keep what
/// they settled as `Stream::run` has them. None of a plan that is a
/// point read, or that the executor opens.
fn lent_rows(cx: StatementContext<'_>, run: &Run, (reads, db): (&[QualifiedName], &Db)) -> Result<Option<HeldRows<Row>>> {
    let Run::Point(kept) = run else { return Ok(None) };
    if kept.point(cx.st).is_some() {
        return Ok(None);
    }
    let db = db.clone().keeping(Arc::clone(kept.subqueries()));
    let Some(lent) = kept.lent().and_then(|plan| Lent::of(plan, &db)) else { return Ok(None) };
    for t in reads {
        cx.st.read_lock(cx.tx, t)?;
    }
    let mut found = HeldRows::new(cx.st.list_space());
    lent.rows(&db, cx.tx.id, &mut |row| found.push(row))?;
    Ok(Some(found))
}
