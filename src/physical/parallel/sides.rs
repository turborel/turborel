//! \[DESIGN\] (RULES.md D1278) How the two forms of a grouping under an
//! ORDER BY of its keys would run, as the planner weighs them
//! (`physical/streamed.rs`) before the marking pass marks either: the
//! threads a hashed grouping of a stream, and a sort of the stream's rows,
//! would each be given, by the marking pass's own rule (RULES.md D291),
//! and the share of each thread past the first that the stream's table
//! lets a part gain (`cost/parallel.rs`, RULES.md D652).

use super::{Asked, MAX_DEPTH, rooted, stream_in};
use crate::db::State;
use crate::physical::cost::parallel::{ROW_STORE_GAIN, gain_of};
use crate::physical::{Plan, Seen};

/// Whether a part reading the stream `input` could run on more threads
/// than one, by the rows it reads alone: of an input that is no stream,
/// or of too few rows, no form of a grouping of it is a part of several.
pub(in crate::physical) fn shares(db: &State, seen: &Seen, input: &Plan) -> bool {
    let asked = Asked::of(db, seen);
    asked.most > 1 && stream_in(input, Some(db)).is_some_and(|rows| asked.by_rows(rows) > 1)
}

/// The threads `plan`, a grouping or DISTINCT, would run on as it is:
/// those of the part it would be marked, one if it is no part or one kept
/// whole on the statement's thread.
pub(in crate::physical) fn part_degree(db: &State, seen: &Seen, plan: &Plan) -> usize {
    let asked = Asked::of(db, seen);
    let Some(rows) = rooted(plan, Some(db)).filter(|_| asked.most > 1) else { return 1 };
    if asked.kept_whole(plan, rows) { 1 } else { asked.degree(plan, rows) }
}

/// The threads a sort of the rows of `input` would run on: those of the
/// part a sort over a stream is, by the rows the stream reads, and one
/// over an input that is no stream, or one a serial scan by batch serves
/// better, as the marking pass leaves such a sort on the statement's
/// thread.
pub(in crate::physical) fn sort_degree(db: &State, seen: &Seen, input: &Plan) -> usize {
    let asked = Asked::of(db, seen);
    stream_in(input, Some(db)).filter(|_| asked.most > 1).map_or(1, |rows| asked.degree(input, rows))
}

/// The share of each thread past the first a part reading the stream
/// `plan` gains: that of the table its morsels are read of, the scan its
/// filters, projections and probes lead down to (`gain_of`). A plan that
/// leads to no one scan, as an append of two does, is taken to gain what
/// a row store's part does, so no plan is chosen for a gain not shown.
pub(in crate::physical) fn stream_gain(db: &State, plan: &Plan) -> f64 {
    let mut at = plan;
    for _ in 0..MAX_DEPTH {
        if let Plan::SeqScan { table, .. } | Plan::IndexScan { table, .. } = at {
            return gain_of(db, table);
        }
        let below: Option<&Plan> = if let Plan::Filter { input, .. } | Plan::Project { input, .. } = at {
            Some(&**input)
        } else if let Plan::HashJoin { left, .. } | Plan::OuterJoin { left, .. } = at {
            Some(&**left)
        } else if let Plan::SemiJoin(s) = at {
            Some(&s.left)
        } else {
            None
        };
        let Some(next) = below else { break };
        at = next;
    }
    ROW_STORE_GAIN
}
