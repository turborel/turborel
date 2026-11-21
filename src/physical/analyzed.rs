//! \[EXTENSION\] What EXPLAIN ANALYZE counts of a plan it runs (RULES.md
//! D376, D378). The plan chosen is wrapped once it is chosen, each operator
//! in a `Counted` that the executor runs as its input, adding the rows the
//! operator gives, the times it is opened and the time it takes to the
//! operator's `NodeCount` (`exec/counted.rs`). A plan made to run is never
//! wrapped, so a statement not analyzed pays nothing for the counting.
//!
//! Two operators are left as they are. A part run in parallel is counted
//! by the operator at its top, as its rows are that operator's. The sort
//! below a window run in parallel is counted by the window, since the part
//! sorts the window's rows itself (`exec/parallel/partitioned.rs`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::Plan;
pub(crate) use super::shown::millis;
use crate::noted::{Notes, NotesRead};

/// \[DESIGN\] How deep a plan is wrapped (RULES.md R-CF-6): deeper than any
/// plan the binder makes nests. An operator below it runs uncounted.
const MAX_DEPTH: usize = 1_024;

/// \[DESIGN\] What one operator did as it ran: the rows it gave, the times
/// it was opened and the microseconds it took, by the database's clock,
/// its inputs' among them. Each thread running it adds what it counted as
/// it finishes, so threads add once each, not once a row.
#[derive(Debug, Default)]
pub struct NodeCount {
    rows: AtomicU64,
    opens: AtomicU64,
    micros: AtomicU64,
    /// Whether rows were counted of it by a step of a pipeline run in
    /// parallel, whose rows pass every step at once, so it has no time of
    /// its own.
    stepped: AtomicBool,
    /// Whether its time is shown, as the statement's clock moved.
    timed: AtomicBool,
    /// What it did beside giving rows (`noted.rs`, RULES.md D620).
    notes: Arc<Notes>,
}

impl NodeCount {
    /// Counts one opening of it.
    pub fn opened(&self) {
        self.opens.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds `rows` rows and `micros` microseconds to it.
    pub fn add(&self, rows: u64, micros: u64) {
        self.rows.fetch_add(rows, Ordering::Relaxed);
        self.micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// Adds `rows` rows counted by a step of a pipeline run in parallel.
    pub fn stepped(&self, rows: u64) {
        self.stepped.store(true, Ordering::Relaxed);
        self.rows.fetch_add(rows, Ordering::Relaxed);
    }

    /// Sets whether its time is shown, by whether the statement's clock moved.
    pub fn show_time(&self, moved: bool) {
        self.timed.store(moved, Ordering::Relaxed);
    }

    /// Its notes, the thread's as it is pulled (`noted::within`).
    pub(crate) fn notes(&self) -> &Arc<Notes> {
        &self.notes
    }

    /// What its notes hold.
    pub fn noted(&self) -> NotesRead {
        self.notes.read()
    }

    /// The rows it gave.
    pub fn rows(&self) -> u64 {
        self.rows.load(Ordering::Relaxed)
    }

    /// The times it was opened.
    pub fn opens(&self) -> u64 {
        self.opens.load(Ordering::Relaxed)
    }

    /// Its microseconds, if they are known and shown.
    pub fn micros(&self) -> Option<u64> {
        let timed = self.timed.load(Ordering::Relaxed) && !self.stepped.load(Ordering::Relaxed);
        timed.then(|| self.micros.load(Ordering::Relaxed))
    }
}

/// \[EXTENSION\] An operator EXPLAIN ANALYZE counts: run as `input`, its
/// rows and time added to `count`.
#[derive(Debug, Clone)]
pub struct Counted {
    /// What it did.
    pub count: Arc<NodeCount>,
    /// The operator.
    pub input: Plan,
}

/// `plan`, each of its operators counted (as the module's documentation
/// says), and each count made.
pub fn counted(plan: Plan) -> (Plan, Vec<Arc<NodeCount>>) {
    let mut made = Vec::new();
    let plan = wrapped(plan, &mut made, 0);
    (plan, made)
}

/// `plan` and its inputs counted, `depth` levels below the top.
fn wrapped(mut plan: Plan, made: &mut Vec<Arc<NodeCount>>, depth: usize) -> Plan {
    if depth > MAX_DEPTH {
        return plan;
    }
    wrap_inputs(&mut plan, made, depth);
    if matches!(plan, Plan::Parallel(_)) {
        return plan;
    }
    counted_node(plan, made)
}

/// `plan`'s inputs counted, it `depth` levels below the top.
fn wrap_inputs(plan: &mut Plan, made: &mut Vec<Arc<NodeCount>>, depth: usize) {
    let next = depth.saturating_add(1);
    if let Plan::Parallel(p) = plan
        && let Plan::Window { window, .. } = &mut p.input
    {
        // The sort's input counted, not the sort.
        wrap_inputs(&mut window.input, made, next.saturating_add(1));
        let top = taken(&mut p.input);
        p.input = counted_node(top, made);
        return;
    }
    plan.each_input(&mut |input| *input = wrapped(taken(input), made, next));
}

/// `plan` counted, its count added to `made`.
fn counted_node(plan: Plan, made: &mut Vec<Arc<NodeCount>>) -> Plan {
    let count = Arc::new(NodeCount::default());
    made.push(Arc::clone(&count));
    Plan::Counted(Box::new(Counted { count, input: plan }))
}

/// The plan in `slot`, an empty list of rows left in its place.
fn taken(slot: &mut Plan) -> Plan {
    std::mem::replace(slot, Plan::Values { rows: Vec::new(), detail: String::new() })
}

impl Plan {
    /// Itself, past the `Counted` around it if one is.
    pub(crate) fn bare(&self) -> &Plan {
        if let Plan::Counted(c) = self { &c.input } else { self }
    }

    /// Whether it is an operator of a join.
    fn is_join(&self) -> bool {
        matches!(
            self,
            Plan::Product { .. }
                | Plan::HashJoin { .. }
                | Plan::IndexJoin { .. }
                | Plan::MergeJoin(_)
                | Plan::RangeJoin(_)
                | Plan::OuterJoin { .. }
                | Plan::SemiJoin(_)
        )
    }

    /// The tables it reads itself, not through an input.
    fn own_tables(&self) -> usize {
        usize::from(matches!(self, Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::IndexJoin { .. } | Plan::Nearest(_)))
    }
}

/// \[EXTENSION\] One operator of a plan EXPLAIN ANALYZE ran, for a tool to
/// read (`Session::analyzed`).
#[derive(Debug, Clone, PartialEq)]
pub struct Operator {
    /// Its line, as EXPLAIN shows it.
    pub label: String,
    /// How many operators it is below the top.
    pub depth: usize,
    /// The rows the planner estimated it gives, if it estimated them.
    pub estimate: Option<f64>,
    /// The rows it gave, every time it was opened.
    pub rows: u64,
    /// The times it was opened.
    pub loops: u64,
    /// The microseconds it took, its inputs' among them, by the database's
    /// clock: none if the clock did not move, or of an operator of a
    /// pipeline run in parallel.
    pub micros: Option<u64>,
    /// The tables read below it, its own among them.
    pub tables: usize,
    /// Whether it joins.
    pub join: bool,
    /// What it spilled, its batches, and what it read of indexes.
    pub notes: NotesRead,
}

/// \[EXTENSION\] What EXPLAIN ANALYZE found of a query it ran: each
/// operator, in the order EXPLAIN shows them, the rows of the query, its
/// time, and the text EXPLAIN ANALYZE shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Analyzed {
    /// The operators counted, the top first, each before its inputs.
    pub operators: Vec<Operator>,
    /// The rows the query gave.
    pub rows: u64,
    /// Its microseconds, by the database's clock, if it moved.
    pub micros: Option<u64>,
    /// The text, a line each.
    pub text: String,
}

/// The operators of `plan`, a counted plan run, each added to `out`, it
/// `depth` levels below the top; the tables it reads.
pub(crate) fn operators(plan: Plan, depth: usize, out: &mut Vec<Operator>) -> usize {
    if depth > MAX_DEPTH {
        return 0;
    }
    let (mut node, at) = match plan {
        Plan::Counted(c) => {
            let Counted { count, input } = *c;
            out.push(operator(&input, &count, depth));
            (input, out.len().checked_sub(1))
        }
        other @ (Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Nearest(_)
        | Plan::Filter { .. }
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::GroupJoin(_)
        | Plan::SemiJoin(_)
        | Plan::RangeJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Unnest(_)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(_)
        | Plan::Parallel(_)) => (other, None),
    };
    let mut tables = node.own_tables();
    let next = depth.saturating_add(1);
    node.each_input(&mut |input| tables = tables.saturating_add(operators(taken(input), next, out)));
    if let Some(op) = at.and_then(|at| out.get_mut(at)) {
        op.tables = tables;
    }
    tables
}

/// The operator `node`, counted by `count`, `depth` levels below the top.
fn operator(node: &Plan, count: &NodeCount, depth: usize) -> Operator {
    let label = node.explain().lines().next().unwrap_or_default().trim().to_owned();
    Operator {
        label,
        depth,
        estimate: node.est(),
        rows: count.rows(),
        loops: count.opens(),
        micros: count.micros(),
        tables: 0,
        join: node.is_join(),
        notes: count.noted(),
    }
}

// Unit tests: which operators are counted (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::Parallel;

    /// A list of no rows.
    fn values() -> Plan {
        Plan::Values { rows: Vec::new(), detail: String::new() }
    }

    /// Verifies: [EXTENSION] RULES.md D378: each operator is counted but a
    /// part run in parallel, whose top operator is; each count is made
    /// once; and what is counted is found again, each operator at its
    /// depth, below a part's mark one deeper.
    #[test]
    fn each_operator_is_counted_but_a_parallel_mark() {
        let distinct = Plan::Distinct { input: Box::new(values()), est: 1.0_f64, streamed: false };
        let part = Plan::Parallel(Box::new(Parallel { degree: 2, input: distinct }));
        let plan = Plan::Sort { keys: Vec::new(), detail: String::new(), input: Box::new(part), most: None };
        let (counted, made) = counted(plan);
        assert_eq!(made.len(), 3);
        let Plan::Counted(sort) = &counted else { panic!("the sort is counted") };
        let Plan::Sort { input, .. } = &sort.input else { panic!("a sort") };
        let Plan::Parallel(p) = &**input else { panic!("the mark is not counted") };
        let Plan::Counted(top) = &p.input else { panic!("the part's top operator is counted") };
        assert!(matches!(top.input, Plan::Distinct { .. }));
        assert!(matches!(counted.bare(), Plan::Sort { .. }) && matches!(values().bare(), Plan::Values { .. }));
        let mut found = Vec::new();
        assert_eq!(operators(counted, 0, &mut found), 0);
        let depths: Vec<usize> = found.iter().map(|o| o.depth).collect();
        assert_eq!(depths, [0, 2, 3]);
        assert!(found.iter().all(|o| o.loops == 0 && !o.join));
    }
}
