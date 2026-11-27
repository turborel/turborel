//! \[DESIGN\] (RULES.md D1215) Which groupings may make only the groups a
//! FETCH FIRST over them asks for. Without ORDER BY the order of a query
//! expression's rows is implementation-dependent [ISO/IEC 9075-2:2023
//! §7.17 GR5a] (US036), and OFFSET and FETCH FIRST keep rows by that order
//! [GR6, GR7d ii], so any of the grouping's rows answer, each a whole
//! group [§7.13 GR2b]. A grouping that keeps the first groups it meets and
//! drops every row of another gives such rows.
//!
//! A row dropped is one no General Rule would have judged in the answer
//! given, yet evaluating it could have raised an exception or a warning
//! (RULES.md D15). So the grouping is one read by batch whose set
//! functions each count rows or take a column, raise nothing whatever
//! rows they are given (`filterable`), and take no column that may hold a
//! null, whose elimination is a warning [§10.9 GR7a]. It groups by some
//! column, as a grouping by none has one group; makes no group before its
//! rows, as grouping sets do; and is not streamed, since one streamed
//! gives each group as its last row passes and reads no further once the
//! count is given.

use super::{Taken, filterable, layout, read_of, typed};
use crate::db::State;
use crate::physical::Plan;

/// Whether `plan`, a grouping, may make only the first groups asked of
/// it, as the module says.
pub(in crate::exec) fn fewer_groups(plan: &Plan, db: &State) -> bool {
    let Plan::Aggregate { input, streamed: false, .. } = plan else { return false };
    let Some(read) = read_of(plan, db).filter(|_| filterable(plan, db)) else { return false };
    let (columns, _) = layout(input, db).unwrap_or_default();
    let nullable = |at: &usize| columns.get(*at).is_none_or(|&c| !typed::never_null(input, c, db));
    !read.taken.iter().any(|t| if let Taken::Column(at) = t { nullable(at) } else { false })
}
