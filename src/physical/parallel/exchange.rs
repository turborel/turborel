//! \[DESIGN\] What a grouping's exchange costs (RULES.md D355, D356): a
//! grouping or DISTINCT run in parallel groups each morsel apart, hands its
//! groups to the partitions, and merges each partition's anew
//! (`exec/parallel/grouped.rs`). A morsel of `m` rows of `G` groups, each
//! as likely, holds `G (1 - (1 - 1/G)^m)` of them, as Cardenas, "Analysis
//! and Performance of Inverted Data Base Structures" (1975), counts the
//! blocks a query touches, so the part's `M` morsels hand over that many
//! pieces each. Its threads share the rows and the work of the pieces, each
//! costing `PIECE_WORKED` rows grouped; but a piece's vectors move through
//! the memory every thread shares as they are handed over and merged,
//! which costs `PIECE_MOVED` rows grouped whatever the threads; and the
//! statement's thread merges the partitions' groups in the order of their
//! first rows, each costing `GROUP_ORDERED`. The part stays serial if that
//! costs the rows' grouping alone as much: of many groups on few threads,
//! each morsel grouped barely shrinks.

use super::super::Plan;
use crate::db::State;
use crate::logical::Expr;

/// \[DESIGN\] What a piece costs its thread, in rows grouped: its group
/// made in its morsel's table, found again by the hash it came with in its
/// partition's, its states added slot to slot, and taken up again by each
/// wave after (`exec/parallel/grouped/waves.rs`). Measured as most of a
/// row and a half of a grouping with set functions, less of DISTINCT.
const PIECE_WORKED: f64 = 1.5_f64;

/// \[DESIGN\] What a piece's vectors cost moving through the memory the
/// threads share, in rows grouped: written by one thread, read by another
/// in order, as columns, none of it allocated a piece at a time.
const PIECE_MOVED: f64 = 0.1_f64;

/// \[DESIGN\] What a group costs the statement's thread, in rows grouped:
/// taken from the heap of the partitions' groups by their first rows,
/// given, each partition having ordered its own.
const GROUP_ORDERED: f64 = 0.4_f64;

/// A count as a float, exact to 2^32.
fn real(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// \[DESIGN\] Whether the exchange of `plan`, a grouping or DISTINCT whose
/// part reads `rows` rows in morsels of `morsel` on `degree` threads, costs
/// as much as its threads gain: of groups estimated by `db`'s statistics
/// of its columns; never of groups not estimated, nor of one group.
pub(super) fn outweighs(db: &State, plan: &Plan, rows: usize, (morsel, degree): (usize, usize)) -> bool {
    let Some(groups) = groups_of(db, plan) else { return false };
    let (r, m, d) = (real(rows), real(morsel.min(rows).max(1)), real(degree.max(1)));
    let groups = groups.clamp(1.0_f64, r.max(1.0_f64));
    let per_morsel = groups * (1.0_f64 - (1.0_f64 - groups.recip()).powf(m));
    let pieces = per_morsel * (r / m).ceil();
    let parallel = (r + pieces * PIECE_WORKED) / d + pieces * PIECE_MOVED + groups * GROUP_ORDERED;
    parallel >= r
}

/// The groups `plan` makes, a grouping by columns or DISTINCT of a
/// projection, as the product of its columns' distinct values; none if one
/// is not known.
fn groups_of(db: &State, plan: &Plan) -> Option<f64> {
    if let Plan::Aggregate { groups, input, .. } = plan {
        let product = groups.iter().try_fold(1.0_f64, |n, &c| Some(n * distinct_in(db, input, c)?));
        return (!groups.is_empty()).then_some(product).flatten();
    }
    let Plan::Distinct { input } = plan else { return None };
    let Plan::Project { exprs, .. } = &**input else { return None };
    (0..exprs.len()).try_fold(1.0_f64, |n, c| Some(n * distinct_in(db, input, c)?))
}

/// The distinct values of column `c` of the stream `plan`, as its table's
/// statistics count them: a filter's as its input's, a projection's of a
/// column as that column's.
fn distinct_in(db: &State, plan: &Plan, c: usize) -> Option<f64> {
    if let Plan::SeqScan { table, .. } = plan {
        return db.statistics(table)?.distinct(c);
    }
    if let Plan::Filter { input, .. } = plan {
        return distinct_in(db, input, c);
    }
    let Plan::Project { exprs, input, .. } = plan else { return None };
    let Some(Expr::Column(at)) = exprs.get(c) else { return None };
    distinct_in(db, input, *at)
}
