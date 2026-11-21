//! \[DESIGN\] The settings of join enumeration (RULES.md D305): which
//! search plans a chain of joins, and the limits of the adaptive one's
//! stages (`physical/enumerate.rs`); whether a join comparing a value
//! of each operand by order may be a range join (`physical/range.rs`,
//! RULES.md D582); whether a grouping may be streamed (D550,
//! `physical/streamed.rs`); how far an estimate is distrusted as a
//! join's method is chosen (`physical/robust.rs`); and how much a grouping
//! over joins with a table grouped first must gain to be planned so
//! (`physical/eager.rs`, RULES.md D663).

use super::builtin::{SESSION, count, defined, text};
use super::{Definition, Key, Kind, ON, SWITCHES};
use crate::physical::{ADAPTIVE, DEFAULT_DP_BUDGET, DEFAULT_EAGER_GAIN, DEFAULT_ESTIMATE_MARGIN, DEFAULT_LINEARIZED, ENUMERATIONS, MOST_ESTIMATE_MARGIN};
use crate::value::Value;

/// PLANNER.JOIN_ENUMERATION.
pub const JOIN_ENUMERATION: Key = Key(79);
/// PLANNER.DP_BUDGET.
pub const DP_BUDGET: Key = Key(80);
/// PLANNER.LINEARIZED_RELATIONS.
pub const LINEARIZED_RELATIONS: Key = Key(81);
/// PLANNER.RANGE_JOINS.
pub const RANGE_JOINS: Key = Key(88);
/// PLANNER.STREAMED_GROUPING.
pub const STREAMED_GROUPING: Key = Key(90);
/// PLANNER.ESTIMATE_MARGIN, at its key's place after the streamed groupings'
/// setting (RULES.md D386).
pub const ESTIMATE_MARGIN: Key = Key(91);
/// PLANNER.EAGER_GAIN, at its key's place after the dense joins' setting.
pub const EAGER_GAIN: Key = Key(93);

/// The most connected subgraphs the budget may be: a million, past which
/// exact DP's table outgrows a planner's memory for one statement.
const MOST_BUDGET: i128 = 1_000_000;
/// The most relations linearized DP may plan whole: the most relations a
/// chain is enumerated of (`enumerate/bits.rs`).
const MOST_LINEARIZED: i128 = 128;
/// \[DESIGN\] The most the eager gain may be: past any ratio of two
/// plans' estimated work, so a session may take every plan grouped first
/// that computes the grouping, as to compare the two.
const MOST_GAIN: f64 = 1_000_000.0;

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

/// The setting of streamed groupings, at its key's place after the
/// range joins'.
pub(super) fn streamed() -> [Definition; 1] {
    [defined(
        ("PLANNER.STREAMED_GROUPING", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a grouping or DISTINCT whose rows come in the order of its keys, or cost less sorted, is made a group at a time as its rows come, with nothing held, ON, or always by a table of its groups, OFF, as to compare the two.",
    )]
}

/// PLANNER.ESTIMATE_MARGIN (RULES.md D386).
pub(super) fn margin() -> Definition {
    defined(
        ("PLANNER.ESTIMATE_MARGIN", Kind::Number { min: 1.0_f64, max: MOST_ESTIMATE_MARGIN }, Value::Approx(DEFAULT_ESTIMATE_MARGIN)),
        SESSION,
        "How many times its estimated rows the planner takes a join's operand to give at worst, at most the bound its statistics prove, as it weighs probing an index for each of them against hashing: it takes the method whose cost is nearest the cheapest at both. 1 trusts the estimate alone.",
    )
}

/// PLANNER.EAGER_GAIN, at its key's place after the dense joins' setting.
pub(super) fn eager_gain() -> Definition {
    defined(
        ("PLANNER.EAGER_GAIN", Kind::Number { min: 0.0, max: MOST_GAIN }, Value::Approx(DEFAULT_EAGER_GAIN)),
        SESSION,
        "How much less work a grouping over joins, planned with one of its tables grouped first by the columns the joins and the grouping read, must be estimated to do than the grouping planned as written, to be planned so. 0 never groups first, and a value past 1 groups first even where that is estimated to cost more, as to compare the two.",
    )
}
