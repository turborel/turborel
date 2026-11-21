//! \[DESIGN\] The settings of join enumeration (RULES.md D305): which
//! search plans a chain of joins, and the limits of the adaptive one's
//! stages (`physical/enumerate.rs`); and whether a join comparing a value
//! of each operand by order may be a range join (`physical/range.rs`,
//! RULES.md D582).

use super::builtin::{SESSION, count, defined, text};
use super::{Definition, Key, Kind, ON, SWITCHES};
use crate::physical::{ADAPTIVE, DEFAULT_DP_BUDGET, DEFAULT_LINEARIZED, ENUMERATIONS};

/// PLANNER.JOIN_ENUMERATION.
pub const JOIN_ENUMERATION: Key = Key(79);
/// PLANNER.DP_BUDGET.
pub const DP_BUDGET: Key = Key(80);
/// PLANNER.LINEARIZED_RELATIONS.
pub const LINEARIZED_RELATIONS: Key = Key(81);
/// PLANNER.RANGE_JOINS.
pub const RANGE_JOINS: Key = Key(88);

/// The most connected subgraphs the budget may be: a million, past which
/// exact DP's table outgrows a planner's memory for one statement.
const MOST_BUDGET: i128 = 1_000_000;
/// The most relations linearized DP may plan whole: the most relations a
/// chain is enumerated of (`enumerate/bits.rs`).
const MOST_LINEARIZED: i128 = 128;

/// The settings of join enumeration, in the order of their keys.
pub(super) fn definitions() -> [Definition; 3] {
    [
        defined(
            ("PLANNER.JOIN_ENUMERATION", Kind::Choice(ENUMERATIONS), text(ADAPTIVE)),
            SESSION,
            "How the joins of a query are ordered. ADAPTIVE orders them by dynamic programming over bushy plans, outer joins among them, linearized or guided greedily as the query grows, and GREEDY orders them left-deep, each next join the cheapest, as to compare the two.",
        ),
        defined(
            ("PLANNER.DP_BUDGET", Kind::Integer { min: 1, max: MOST_BUDGET }, count(DEFAULT_DP_BUDGET)),
            SESSION,
            "The connected subsets of a query's joined tables up to which ADAPTIVE join ordering finds the cheapest plan exactly. Past it the search is linearized, and parts of a greedy plan are planned again by dynamic programming until this many table entries are made.",
        ),
        defined(
            ("PLANNER.LINEARIZED_RELATIONS", Kind::Integer { min: 2, max: MOST_LINEARIZED }, count(DEFAULT_LINEARIZED)),
            SESSION,
            "The most tables ADAPTIVE join ordering plans whole by linearized dynamic programming when exact search is past its budget, and the size of the parts of a greedy plan it plans again so for larger queries.",
        ),
    ]
}

/// PLANNER.RANGE_JOINS, at its key's place after the executor's settings.
pub(super) fn range_joins() -> Definition {
    defined(
        ("PLANNER.RANGE_JOINS", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a join whose condition compares a value of each table by <, <=, > or >=, a band or an overlap of periods among them, may sort one table and have each row of the other search it, rather than judge every pair. OFF plans such joins as nested loops, as to compare the two.",
    )
}
