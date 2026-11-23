//! \[DESIGN\] Direct range reads (RULES.md D930). A kept plan that reads
//! one range of an index, under filters, projections, a sort, OFFSET and
//! FETCH FIRST or a grouping by no column, is run where it is kept, with
//! no copy of it made and no operator built: its statement's values are
//! read where they are, its range is read direct (`range/scan.rs`), and
//! each row is handed up through what the plan puts over the range, as
//! the executor's operators would hand it. A filter judges the row, a
//! projection makes its row of it, FETCH FIRST stops the read once it has
//! its rows, and set functions fold it (`range/fold.rs`). A sort holds the
//! rows read and sorts them as the executor sorts (`sort.rs`), and a
//! grouping by a column holds them and groups them as it groups
//! (`group.rs`). An inner join that probes a whole unique key for each
//! row of the range pairs it with the row a point read finds
//! (`join/indexed/direct.rs`).
//!
//! Each part does what its operator does, by the functions the operator
//! evaluates with, so the rows, the errors and the reads recorded are the
//! plan's. A plan of any other shape, or one whose expressions read beyond
//! their row, a subquery, a routine or a sequence generator, is no direct
//! read, and the executor runs it.

mod fold;
mod scan;

use std::sync::Mutex;

use super::join::{DirectJoin, Given, Pairing};
use super::limit::{Rule, count};
use super::{Db, Subqueries, broken, eval, eval_cond, group, sort};
use crate::db::State;
use crate::error::Result;
use crate::logical::{AggCall, Cond, Expr, Limit, Most, SortKey, Truth};
use crate::physical::Plan;
use crate::storage::{Row, sized_row};
use crate::txn::TxId;
use crate::value::Value;
use fold::Fold;
use scan::Scan;

/// \[DESIGN\] How many operators deep a plan read direct may be: a range
/// under each of the operators it may be under, twice over, which bounds
/// the recursion of `Node::of` and `Node::run` (RULES.md R-CF-6).
const MOST_DEPTH: usize = 12;

/// Where a direct read runs: the state and transaction it reads in, its
/// statement's slot values, and what evaluates its expressions.
pub(crate) struct At<'a> {
    pub(crate) st: &'a std::sync::Arc<State>,
    pub(crate) tx: TxId,
    pub(crate) values: &'a [Value],
    pub(crate) subqueries: &'a dyn Subqueries,
}

/// What takes each row of a part of a direct read, lent: it may take the
/// row's values, and says whether it wants another.
pub(crate) type Each<'e> = &'e mut dyn FnMut(&mut Row) -> Result<bool>;

/// What a part of a direct read did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ran {
    /// It gave its rows.
    Done,
    /// Nothing: it is not read direct with these values, and the executor
    /// runs its plan.
    Unread,
}

/// \[DESIGN\] A kept plan run as a direct range read (RULES.md D930).
#[derive(Debug, Clone)]
pub(crate) struct RangeRead {
    node: Node,
    held: KeptHeld,
    /// Whether its plan can raise an exception of a General Rule
    /// (`Plan::can_raise`).
    raises: bool,
}

/// What a direct read keeps from one run to the next (`scan::Held`), held
/// with the plan. One execution takes it at a time: another running at
/// once, or under it, reads with one of its own. A copy of the plan
/// starts with none.
#[derive(Default)]
struct KeptHeld(Mutex<scan::Held>);

impl Clone for KeptHeld {
    fn clone(&self) -> KeptHeld {
        KeptHeld::default()
    }
}

impl std::fmt::Debug for KeptHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeptHeld")
    }
}

/// A part of a plan read direct.
#[derive(Debug, Clone)]
enum Node {
    Scan(Box<Scan>),
    Filter(Cond, Box<Node>),
    /// The expressions; of columns alone, each column and whether a later
    /// one names it again, so it is copied; and whether none can raise.
    Project(Projection, Box<Node>),
    /// The counts, and whether its input can raise, so is read to its end.
    Limit(Limit, bool, Box<Node>),
    Sort(Vec<SortKey>, Option<Most>, Box<Node>),
    Fold(Fold, Box<Node>),
    /// A grouping `Fold` does not fold, and whether its rows come in the
    /// order of its grouping columns (`Plan::Aggregate`).
    Group(Grouping, bool, Box<Node>),
    /// An index join by a whole unique key, over its left operand.
    Join(Box<DirectJoin>, Box<Node>),
}

/// A grouping of a direct read: its grouping columns, their collations,
/// its set functions and the groups it makes before any row.
#[derive(Debug, Clone)]
struct Grouping {
    groups: Vec<usize>,
    collations: Vec<Option<crate::value::collation::CollationRef>>,
    aggs: Vec<AggCall>,
    seeds: Vec<Row>,
}

/// A projection of a direct read.
#[derive(Debug, Clone)]
struct Projection {
    exprs: Vec<Expr>,
    /// Of columns alone: each, and whether one after it names it again.
    columns: Option<Vec<(usize, bool)>>,
    /// Whether no expression of it can raise.
    quiet: bool,
}

impl Projection {
    /// The projection of `exprs`.
    fn of(exprs: &[Expr]) -> Projection {
        let named = |e: &Expr| if let Expr::Column(c) = e { Some(*c) } else { None };
        let columns = exprs.iter().map(named).collect::<Option<Vec<usize>>>();
        let again = |all: &[usize], at: usize, c: usize| all.iter().skip(at.saturating_add(1)).any(|later| *later == c);
        let columns = columns.map(|all| all.iter().enumerate().map(|(at, &c)| (c, again(&all, at, c))).collect());
        Projection { exprs: exprs.to_vec(), columns, quiet: !exprs.iter().any(Expr::can_raise) }
    }

    /// The row it makes of `row`, whose values it may take.
    fn row(&self, row: &mut Row, subqueries: &dyn Subqueries) -> Result<Row> {
        let Some(columns) = &self.columns else {
            return sized_row(self.exprs.len(), self.exprs.iter().map(|e| eval(e, row, subqueries)));
        };
        let width = row.len();
        let taken = columns.iter().map(|&(at, again)| {
            let value = row.get_mut(at).ok_or_else(|| broken(&format!("column {at} is not in a row of {width} columns")))?;
            Ok(if again { value.clone() } else { std::mem::replace(value, Value::Null) })
        });
        sized_row(columns.len(), taken)
    }
}

/// Whether `cond` reads its row alone: no subquery, routine or sequence
/// generator.
fn own(cond: &Cond) -> bool {
    let mut own = !cond.has_subquery();
    cond.for_each_expr(&mut |e| own = own && e.reads_nothing());
    own
}

impl Node {
    /// The part `plan` is, in `st`, no more than `depth` operators deep,
    /// if it is read direct.
    fn of(plan: &Plan, st: &State, depth: usize) -> Option<Node> {
        let under = |input: &Plan| Node::of(input, st, depth.checked_sub(1)?).map(Box::new);
        if let Plan::Filter { cond, input, .. } = plan {
            return own(cond).then(|| under(input)).flatten().map(|input| Node::Filter(cond.clone(), input));
        }
        if let Plan::Project { exprs, input, .. } = plan {
            let read = exprs.iter().all(Expr::reads_nothing);
            return read.then(|| under(input)).flatten().map(|input| Node::Project(Projection::of(exprs), input));
        }
        if let Plan::Limit { limit, input, .. } = plan {
            // A percentage holds every row, and WITH TIES reads past its count.
            let counted = limit.fetch.as_ref().is_none_or(|f| !f.percent && f.ties.is_none());
            let read = limit.exprs().all(Expr::reads_nothing);
            return (counted && read).then(|| under(input)).flatten().map(|under| Node::Limit(limit.clone(), input.can_raise(), under));
        }
        if let Plan::Sort { keys, input, most, .. } = plan {
            return under(input).map(|input| Node::Sort(keys.clone(), *most, input));
        }
        if let Plan::Aggregate { groups, collations, aggs, seeds, input, streamed, .. } = plan {
            if let Some(fold) = Fold::of(plan) {
                return under(input).map(|input| Node::Fold(fold, input));
            }
            let read = aggs.iter().all(|a| a.exprs().into_iter().all(Expr::reads_nothing) && a.filter.as_ref().is_none_or(own));
            let grouping = Grouping { groups: groups.clone(), collations: collations.clone(), aggs: aggs.clone(), seeds: seeds.clone() };
            return read.then(|| under(input)).flatten().map(|input| Node::Group(grouping, *streamed, input));
        }
        if let Plan::IndexJoin { left, .. } = plan {
            let left = under(left)?;
            return DirectJoin::of(plan, &left.given(st)?, st).map(|join| Node::Join(Box::new(join), left));
        }
        Scan::of(plan, st).map(|scan| Node::Scan(Box::new(scan)))
    }

    /// What each column of its rows gives, if each is a column of its
    /// range's table as it is, a constant or one of the statement's
    /// values: of a range, filtered or projected to those.
    fn given(&self, st: &State) -> Option<Vec<Given>> {
        match self {
            Node::Scan(scan) => Some(st.catalog.table(scan.table())?.columns.iter().map(|c| Given::Typed(c.data_type.clone())).collect()),
            Node::Filter(_, input) => input.given(st),
            Node::Project(projection, input) => {
                let under = input.given(st)?;
                let given = |e: &Expr| match e {
                    Expr::Column(c) => under.get(*c).cloned(),
                    Expr::Const(value) => Some(Given::Constant(value.clone())),
                    // A slot's value is cast to its type as it is read.
                    Expr::Dynamic(slot) => Some(Given::Typed(slot.data_type.clone())),
                    Expr::Arith(..)
                    | Expr::Neg(_)
                    | Expr::Outer(..)
                    | Expr::Call(..)
                    | Expr::Cast(..)
                    | Expr::Case(..)
                    | Expr::Subquery(..)
                    | Expr::SortKey(..)
                    | Expr::Domain(..)
                    | Expr::Warned(..)
                    | Expr::Truth(_)
                    | Expr::NextValue(_)
                    | Expr::Invoked(_) => None,
                };
                projection.exprs.iter().map(given).collect()
            }
            Node::Limit(..) | Node::Sort(..) | Node::Fold(..) | Node::Group(..) | Node::Join(..) => None,
        }
    }

    /// Lends `each` its rows, in its order, until it wants no more: or
    /// says nothing was read. `most`, if given, is how many of its first
    /// rows are asked for, which a sort then holds alone (`limit/bounded.rs`).
    fn run(&self, at: &At<'_>, held: &mut scan::Held, most: Option<Most>, each: Each<'_>) -> Result<Ran> {
        match self {
            Node::Scan(scan) => scan.run(at, held, each),
            Node::Filter(cond, input) => input.run(at, held, None, &mut |row| {
                if eval_cond(cond, row, at.subqueries)? == Truth::True { each(row) } else { Ok(true) }
            }),
            Node::Project(projection, input) => {
                input.run(at, held, most.filter(|_| projection.quiet), &mut |row| each(&mut projection.row(row, at.subqueries)?))
            }
            Node::Limit(limit, drain, input) => limited((limit, *drain), input, (at, held), each),
            Node::Sort(keys, own, input) => sorted((keys, most.or(*own)), input, (at, held), each),
            Node::Fold(fold, input) => {
                let mut folding = fold.begun(at);
                let ran = input.run(at, held, None, &mut |row| fold.feed(&mut folding, row, at).map(|()| true))?;
                if ran == Ran::Done {
                    each(&mut folding.finished()?)?;
                }
                Ok(ran)
            }
            Node::Group(grouping, streamed, input) => grouped((grouping, *streamed), input, (at, held), each),
            Node::Join(join, input) => {
                let mut pairing = Pairing::default();
                input.run(at, held, None, &mut |left| if join.pair(at, left, &mut pairing)? { each(pairing.row()) } else { Ok(true) })
            }
        }
    }
}

/// Every row of `input`, taken: none if nothing was read.
fn taken(input: &Node, (at, held): (&At<'_>, &mut scan::Held)) -> Result<Option<Vec<Result<Row>>>> {
    let mut rows: Vec<Result<Row>> = Vec::new();
    let ran = input.run(at, held, None, &mut |row| {
        rows.push(Ok(std::mem::take(row)));
        Ok(true)
    })?;
    Ok((ran == Ran::Done).then_some(rows))
}

/// [X3.135-1986 §5.22, §5.8] The groups of the rows of `input` and their
/// set functions' values, lent to `each`: the rows read first, then
/// grouped as the executor groups them (`group.rs`), which reads the
/// statement's values where they are.
fn grouped((grouping, streamed): (&Grouping, bool), input: &Node, (at, held): (&At<'_>, &mut scan::Held), each: Each<'_>) -> Result<Ran> {
    let Some(rows) = taken(input, (at, held))? else { return Ok(Ran::Unread) };
    let Grouping { groups, collations, aggs, seeds } = grouping.clone();
    let spec = group::Spec { groups, collations, aggs, seeds };
    let db = Db::slotted(at.st, at.values.into());
    let read = Box::new(rows.into_iter());
    let made = if streamed { group::streamed_grouping(spec, read, &db, at.tx) } else { group::grouping(spec, read, &db, at.tx) };
    for row in made {
        if !each(&mut row?)? {
            break;
        }
    }
    Ok(Ran::Done)
}

/// [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] The rows of `input` that `limit`
/// keeps, lent to `each`: its counts evaluated first, then OFFSET's rows
/// passed over and the rest kept up to FETCH FIRST's count, after which
/// no row is read, but of an input that can raise, read to its end
/// (`limit.rs`).
fn limited((limit, drain): (&Limit, bool), input: &Node, (at, held): (&At<'_>, &mut scan::Held), each: Each<'_>) -> Result<Ran> {
    let mut skip = match &limit.offset {
        Some(e) => count(e, at.subqueries, &Rule::OFFSET)?,
        None => 0,
    };
    let mut left = match &limit.fetch {
        Some(f) => Some(count(&f.count, at.subqueries, &Rule::FETCH)?),
        None => None,
    };
    let most = left.and_then(|left| skip.checked_add(left)).map(|rows| Most { rows, ties: false });
    input.run(at, held, most, &mut |row| {
        if left == Some(0) {
            return Ok(drain);
        }
        if skip > 0 {
            skip = skip.saturating_sub(1);
            return Ok(true);
        }
        left = left.map(|n| n.saturating_sub(1));
        Ok(each(row)? && (left != Some(0) || drain))
    })
}

/// [X3.135-1986 §8.3 GR3b] The rows of `input` sorted by `keys`, or the
/// first of them if `most` says how many are asked for, lent to `each`:
/// read first, then sorted as the executor sorts them (`sort.rs`).
fn sorted((keys, most): (&[SortKey], Option<Most>), input: &Node, (at, held): (&At<'_>, &mut scan::Held), each: Each<'_>) -> Result<Ran> {
    let Some(rows) = taken(input, (at, held))? else { return Ok(Ran::Unread) };
    let read = Box::new(rows.into_iter());
    let sorted = match most {
        Some(most) => sort::first(keys, read, most, at.st, at.tx),
        None => sort::sort(keys, read, at.st, at.tx),
    };
    for row in sorted {
        if !each(&mut row?)? {
            break;
        }
    }
    Ok(Ran::Done)
}

impl RangeRead {
    /// The direct read `plan` is, in `st`, the state it was planned in, if
    /// it is one.
    pub(crate) fn of(plan: &Plan, st: &State) -> Option<RangeRead> {
        Node::of(plan, st, MOST_DEPTH).map(|node| RangeRead { node, held: KeptHeld::default(), raises: plan.can_raise() })
    }

    /// Whether its plan can raise an exception of a General Rule, so a
    /// cursor of it is read in full as it opens (`db/stream.rs`).
    pub(crate) fn raises(&self) -> bool {
        self.raises
    }

    /// Whether it reads every row of its range before it gives one: a
    /// sort's rows, or a grouping's.
    pub(crate) fn holds(&self) -> bool {
        let mut node = &self.node;
        loop {
            node = match node {
                Node::Scan(_) => return false,
                Node::Sort(..) | Node::Fold(..) | Node::Group(..) => return true,
                Node::Filter(_, input) | Node::Project(_, input) | Node::Limit(_, _, input) | Node::Join(_, input) => input,
            };
        }
    }

    /// Lends `each` the rows `at`'s transaction sees of the plan, in its
    /// order, until it wants no more: or says nothing was read, and the
    /// executor is to run the plan.
    ///
    /// # Errors
    ///
    /// The plan's, as the executor raises them.
    pub(crate) fn run(&self, at: &At<'_>, each: Each<'_>) -> Result<Ran> {
        let (mut kept, mut own) = (self.held.0.try_lock().ok(), scan::Held::default());
        self.node.run(at, kept.as_deref_mut().unwrap_or(&mut own), None, each)
    }

    /// The first `limit` rows of the plan, or all of them, as `run` reads
    /// them: none if nothing was read.
    ///
    /// # Errors
    ///
    /// As `run`.
    pub(crate) fn rows(&self, at: &At<'_>, limit: Option<usize>) -> Result<Option<Vec<Row>>> {
        let mut rows = Vec::new();
        let ran = self.run(at, &mut |row| {
            if limit == Some(0) {
                return Ok(false);
            }
            rows.push(std::mem::take(row));
            Ok(limit.is_none_or(|n| rows.len() < n))
        })?;
        Ok((ran == Ran::Done).then_some(rows))
    }
}

// Unit tests: which plans are read direct, and their rows beside the
// executor's (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
