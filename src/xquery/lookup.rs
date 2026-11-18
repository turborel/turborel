//! \[EXTENSION\] RULES.md D106: a query reduced to the query of an XML
//! index (`index::xml::XmlQuery`) that selects every value of one node
//! the query's value is not empty of [ISO/IEC 9075-14:2006 draft §8.4 GR2],
//! so a planner may read only the rows the index gives, each checked by
//! the query itself, or answered by the index where it is exact.
//!
//! A query reduces if its body is one path [XQuery §3.2] from the context
//! item or `/` whose steps `steps.rs` takes: of the child, descendant and
//! attribute axes over names, `*` or `node()`, `//`, and of the self axis.
//! Each step's predicates are conditions of its node, their conjuncts
//! [XQuery §3.6] each a relative path of such steps, or a general or value
//! comparison of one, or of `.`, with a literal [XQuery §3.5.1, §3.5.2]
//! (`conditions.rs`). Anything else reduces to none: a planner then reads
//! every row.
//!
//! The path's steps are the lookup's. The last step's comparisons of `.`
//! are the lookup's comparisons; else one of its other conditions, a
//! relative path whose steps are child steps if one is, continues the
//! lookup, as `a[b op v]` selects an `a` with a `b` the comparison holds
//! of, as `a/b op v` does: its lookup is `a/b` and the comparison. Every
//! other condition is a filter of the node of its step
//! (`index::xml::XmlFilter`), as in `/BOOK[@ISBN = "1"]/SECTION`. A
//! condition of `.` of a step before the last reduces to none.
//!
//! A general comparison with a string literal, by the codepoint
//! collation, or with a number is kept, as an index keys a node's value
//! as a string and as an `xs:double` (RULES.md D106); any other is left
//! to the check of each row. A comparison with a number may fail to cast a
//! node's value [err:FORG0001], and a value comparison of more than one
//! node, or of a string with a number, fails [err:XPTY0004]: the lookup of
//! its path, the filter's or the query's own, gives every row it may fail
//! of (`Reduction::raising`).

mod conditions;
mod steps;

use super::Query;
use super::plan::{Op, PathOp};
use crate::index::xml::{MAX_FILTERS, MAX_STEPS, PathPattern, Target, XmlFilter, XmlLookup, XmlQuery};
use conditions::Branch;
use steps::{Twig, twig};

/// \[EXTENSION\] RULES.md D106: a query reduced to an XML index's query.
#[derive(Debug, Clone)]
pub struct Reduction {
    /// The index's query: of a value of one node, a row none of its
    /// lookups gives (`XmlQuery::lookups`) is one the query's value is
    /// empty of with no error, unless it may raise one (`raising`).
    pub query: XmlQuery,
    /// Whether the query's evaluation of a value of one node raises no
    /// error: of no comparison but those with a string. A row such a
    /// query may fail of, the lookups `raising` give (RULES.md D15).
    pub raise_free: bool,
    /// Whether the index's query is the query's own test, its every
    /// comparison kept and raising no error, its every filter's anchor
    /// fixed (`XmlQuery::anchors`), so a row the index answers holds of it
    /// needs no check (`XmlQuery::matches`).
    pub exact: bool,
    /// The places among the query's lookups (`XmlQuery::lookups`) of those
    /// of a comparison that may raise an error: the rows every row the
    /// query may fail of is among, with every row it holds of. None if it
    /// raises none: a row it holds of is then one every lookup gives.
    pub raising: Vec<usize>,
}

impl Query {
    /// The query reduced to an XML index's query, if it is a path the
    /// module's documentation describes; none if not, or if its prolog
    /// declares a variable or a function.
    #[must_use]
    pub fn reduced(&self) -> Option<Reduction> {
        let plan = &self.0;
        if !plan.globals.is_empty() || !plan.functions.is_empty() {
            return None;
        }
        let Op::Path(path) = &plan.body.op else { return None };
        reduce(path)
    }
}

/// A condition of a step, a branch, with the step it is of: 0 the item.
type Placed = (usize, Branch);

/// A path reduced, from the context item or `/`, which of a value's one
/// node is that node.
fn reduce(path: &PathOp) -> Option<Reduction> {
    let Twig { root, steps, target, exact } = twig(path)?;
    let mut placed: Vec<Placed> = Vec::new();
    for condition in root {
        placed.extend(condition.branches()?.into_iter().map(|b| (0, b)));
    }
    let mut lookup = Vec::with_capacity(steps.len());
    for (at, taken) in (1..).zip(steps) {
        lookup.push(taken.step);
        for condition in taken.conditions {
            placed.extend(condition.branches()?.into_iter().map(|b| (at, b)));
        }
    }
    let mut reduction = Reduction {
        query: XmlQuery {
            lookup: XmlLookup { pattern: PathPattern { steps: lookup, target }, comparisons: Vec::new(), row: None },
            filters: Vec::new(),
        },
        raise_free: true,
        exact,
        raising: Vec::new(),
    };
    let last = continued(&mut reduction, &mut placed)?;
    for (at, branch) in placed {
        filtered(&mut reduction, (at, branch), last)?;
    }
    finished(reduction)
}

/// The last step's conditions of `.` made the lookup's comparisons, or
/// else one of its other conditions, of child steps if one is, made the
/// rest of the lookup's path; the last step's place, taken from `placed`.
fn continued(reduction: &mut Reduction, placed: &mut Vec<Placed>) -> Option<usize> {
    let last = reduction.query.lookup.pattern.steps.len();
    let own = |(at, branch): &Placed| *at == last && branch.pattern.steps.is_empty();
    let selves: Vec<Placed> = placed.extract_if(.., |p| own(p)).collect();
    if !selves.is_empty() {
        if last == 0 {
            return None;
        }
        for (_, branch) in selves {
            kept(reduction, 0, branch);
        }
        return Some(last);
    }
    let of_last = |(at, _): &Placed| *at == last;
    let child = |(at, branch): &Placed| *at == last && branch.child_steps();
    let Some(chosen) = placed.iter().position(child).or_else(|| placed.iter().position(of_last)) else {
        return (last > 0).then_some(last);
    };
    let (_, branch) = placed.remove(chosen);
    if reduction.query.lookup.pattern.target == Target::Attribute {
        return None;
    }
    let pattern = &mut reduction.query.lookup.pattern;
    pattern.steps.extend(branch.pattern.steps.iter().cloned());
    pattern.target = branch.pattern.target;
    kept(reduction, 0, branch);
    Some(last)
}

/// `branch`'s comparisons made those of the lookup `lookup`, of the
/// query's lookups (`XmlQuery::lookups`): the query's own, 0, or a
/// filter's, and its kind of comparison noted.
fn kept(reduction: &mut Reduction, lookup: usize, branch: Branch) {
    reduction.exact &= !branch.dropped;
    if branch.raises {
        reduction.raise_free = false;
        if !reduction.raising.contains(&lookup) {
            reduction.raising.push(lookup);
        }
    }
    let comparisons = match lookup.checked_sub(1).and_then(|f| reduction.query.filters.get_mut(f)) {
        Some(filter) => &mut filter.comparisons,
        None => &mut reduction.query.lookup.comparisons,
    };
    comparisons.extend(branch.comparisons);
}

/// `branch`, a condition of the node of step `at`, made a filter of the
/// query; none if it is of `.` and not of the last step, `last`.
fn filtered(reduction: &mut Reduction, (at, branch): Placed, last: usize) -> Option<()> {
    if branch.pattern.steps.is_empty() && at != last {
        return None;
    }
    let pattern = branch.pattern.clone();
    reduction.query.filters.push(XmlFilter { at, pattern, comparisons: Vec::new() });
    let filter = reduction.query.filters.len();
    kept(reduction, filter, branch);
    Some(())
}

/// The reduction, its exactness of its filters' anchors too; none if a
/// lookup of it is past `MAX_STEPS` steps, or it has more filters than
/// `MAX_FILTERS`.
fn finished(mut reduction: Reduction) -> Option<Reduction> {
    let query = &reduction.query;
    if query.filters.len() > MAX_FILTERS || query.lookups().iter().any(|l| l.pattern.steps.len() > MAX_STEPS) {
        return None;
    }
    reduction.exact &= reduction.raise_free && query.anchors().is_ok();
    Some(reduction)
}

// Unit tests: each shape reduced, or not (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
