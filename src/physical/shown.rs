//! \[DESIGN\] What EXPLAIN shows of an operator: text made of the names of
//! its input's columns, when a plan is made to be shown; none, when it is
//! made to run, so a statement run pays nothing for it; and a plan written
//! as EXPLAIN writes it, an operator a line.

use std::fmt::Write;

use super::{PhysicalPlanner, Plan};
use crate::logical::{LogicalPlan, Unnest};
use crate::noted::NotesRead;

/// The names an operator's description is made of, if the plan is shown.
#[derive(Clone, Copy)]
pub(super) struct Shown<'a> {
    names: Option<&'a [String]>,
}

impl<'a> Shown<'a> {
    /// Of `names`, given if the plan is shown.
    pub(super) fn of(names: Option<&'a [String]>) -> Shown<'a> {
        Shown { names }
    }

    /// The names, if the plan is shown.
    pub(super) fn names(self) -> Option<&'a [String]> {
        self.names
    }

    /// `render` of the names, if the plan is shown; else nothing.
    pub(super) fn text(self, render: impl FnOnce(&[String]) -> String) -> String {
        self.names.map(render).unwrap_or_default()
    }
}

impl PhysicalPlanner<'_> {
    /// `names`, an input's column names, as the operators over it are
    /// described by: not at all, of a plan to run.
    pub(super) fn showing<'n>(&self, names: &'n [String]) -> Shown<'n> {
        Shown::of(self.shown.then_some(names))
    }

    /// The names of `input`'s columns, if the operators over it are
    /// described.
    pub(super) fn names_of(&self, input: &LogicalPlan) -> Option<Vec<String>> {
        self.shown.then(|| input.names())
    }

    /// `render` of the names of `input`'s columns, if the operators over it
    /// are described; else nothing.
    pub(super) fn described(&self, input: &LogicalPlan, render: impl FnOnce(&[String]) -> String) -> String {
        if self.shown { render(&input.names()) } else { String::new() }
    }
}

impl Plan {
    /// The plan as indented text, one operator per line.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write_index_join(&self, out: &mut String, pad: &str) -> std::fmt::Result {
        let Plan::IndexJoin { spatial, outer, table, index, method, detail, est, cost, .. } = self else { return Ok(()) };
        let kind = match outer {
            super::Preserved::Inner => "",
            super::Preserved::Outer => "LeftOuter",
            super::Preserved::Both(_) => "FullOuter",
            super::Preserved::Semi => "Semi",
            super::Preserved::Anti => "Anti",
        };
        match spatial.as_deref() {
            Some(super::Spatial::Adaptive) => writeln!(
                out,
                "{pad}AdaptiveSpatialJoin {table} using {index} [{method}]: {detail}  (IndexNestedLoop, bounded boundary-mask scan or envelope partitions)"
            ),
            Some(super::Spatial::Paired(paired)) => writeln!(
                out,
                "{pad}{kind}SynchronizedSpatialJoin {table} using {index} with {} using {} [{method}]: {detail}  (est. rows each={est:.0}, cost={cost:.1})",
                paired.table, paired.index
            ),
            None => {
                // [GS §16 R16.2] Of an adjacency index, which Expand it is (`indexed/expand.rs`).
                let (operator, method) = method
                    .split_once(' ')
                    .filter(|(m, _)| *m == crate::index::ADJACENCY_METHOD)
                    .map_or(("IndexNestedLoop", &**method), |(m, expand)| (expand, m));
                writeln!(out, "{pad}{kind}{operator} {table} using {index} [{method}]: {detail}  (est. rows each={est:.0}, cost={cost:.1})")
            }
        }
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        // Writing to a String cannot fail.
        let _ = match self {
            Plan::SeqScan { table, rows, cost, order, backward, .. } => {
                let ordered = scan_order(order, *backward);
                writeln!(out, "{pad}SeqScan {table}{ordered}  (rows={rows}, cost={cost:.1})")
            }
            Plan::IndexScan { table, index, method, detail, est, cost, reverse, covering, .. } => writeln!(
                out,
                "{pad}{} {table} using {index} [{method}]{}: {detail}  (est. rows={est:.0}, cost={cost:.1})",
                if *covering { "IndexOnlyScan" } else { "IndexScan" },
                if *reverse { " backward" } else { "" }
            ),
            Plan::Filter { detail, of_read: false, .. } => writeln!(out, "{pad}Filter {detail}"),
            Plan::Nearest(read) => writeln!(out, "{pad}{}", nearest_line(read)),
            Plan::Filter { detail, est, of_read: true, .. } => writeln!(out, "{pad}Filter {detail}  (est. rows={est:.0})"),
            Plan::Product { .. } => writeln!(out, "{pad}NestedLoop (Cartesian product)"),
            Plan::HashJoin { detail, .. } => writeln!(out, "{pad}HashJoin {detail}"),
            Plan::MergeJoin(m) => writeln!(out, "{pad}{}MergeJoin {}", if m.outer { "Left" } else { "" }, m.detail),
            Plan::RangeJoin(r) => writeln!(out, "{pad}{}RangeJoin {}", if r.outer { "Left" } else { "" }, r.detail),
            Plan::OuterJoin { kind, detail, .. } => writeln!(out, "{pad}{kind:?}OuterJoin {detail}"),
            Plan::SemiJoin(s) => writeln!(out, "{pad}{} {}", s.operator(), s.detail),
            Plan::IndexJoin { .. } => self.write_index_join(out, &pad),
            Plan::Aggregate { detail, streamed, .. } => writeln!(out, "{pad}Aggregate{} {detail}", streamed_word(*streamed)),
            Plan::Project { detail, .. } => writeln!(out, "{pad}Project {detail}"),
            Plan::Distinct { streamed, .. } => writeln!(out, "{pad}Distinct{}", streamed_word(*streamed)),
            Plan::Append { .. } => writeln!(out, "{pad}Append"),
            Plan::SetOp { op, all, .. } => writeln!(out, "{pad}{op:?}{}", if *all { " all" } else { "" }),
            Plan::Values { detail, .. } => writeln!(out, "{pad}Values {detail}"),
            Plan::System { read } => writeln!(out, "{pad}System {}", read.name()),
            Plan::Sort { detail, .. } => writeln!(out, "{pad}Sort {detail}"),
            Plan::Limit { limit, .. } => writeln!(out, "{pad}{}", limit.render()),
            Plan::Recursive { member, fixpoint } => writeln!(out, "{pad}Recursive member {member} ({})", fixpoint.word()),
            Plan::WorkTable { member, .. } => writeln!(out, "{pad}WorkTable of member {member}"),
            Plan::Transition { .. } => writeln!(out, "{pad}Transition table"),
            Plan::Unnest(u) => write_unnest(u, out, &pad),
            Plan::Window { detail, .. } | Plan::Recognize { detail, .. } | Plan::MergeAppend { detail, .. } => {
                writeln!(out, "{pad}{detail}")
            }
            Plan::Polymorphic(p) => writeln!(out, "{pad}Polymorphic table function {}", p.function()),
            Plan::Parallel(p) => writeln!(out, "{pad}Parallel  (degree={})", p.degree),
            Plan::Counted(c) => return write_counted(c, out, depth),
        };
        self.write_inputs(out, depth.saturating_add(1));
    }

    /// Writes its inputs, each at `depth`.
    fn write_inputs(&self, out: &mut String, depth: usize) {
        match self {
            Plan::SeqScan { .. }
            | Plan::Nearest(_)
            | Plan::IndexScan { .. }
            | Plan::Values { .. }
            | Plan::System { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Counted(_) => {}
            Plan::Recursive { fixpoint, .. } => {
                let (first, second, rest) = fixpoint.plans();
                for p in first.into_iter().chain(second).chain(rest) {
                    p.write(out, depth);
                }
            }
            Plan::Product { left, right, .. }
            | Plan::HashJoin { left, right, .. }
            | Plan::OuterJoin { left, right, .. }
            | Plan::Append { left, right, .. }
            | Plan::SetOp { left, right, .. } => {
                left.write(out, depth);
                right.write(out, depth);
            }
            Plan::Parallel(p) => p.input.write(out, depth),
            Plan::Filter { input, .. }
            | Plan::Aggregate { input, .. }
            | Plan::Project { input, .. }
            | Plan::Distinct { input, .. }
            | Plan::Sort { input, .. }
            | Plan::Limit { input, .. }
            | Plan::IndexJoin { left: input, .. } => input.write(out, depth),
            Plan::Unnest(u) => u.input.write(out, depth),
            Plan::Window { window, .. } => window.input.write(out, depth),
            Plan::Recognize { recognize, .. } => recognize.input.write(out, depth),
            Plan::Polymorphic(p) => p.inputs.iter().for_each(|input| input.write(out, depth)),
            Plan::MergeAppend { inputs, .. } => inputs.iter().for_each(|input| input.write(out, depth)),
            Plan::MergeJoin(m) => {
                m.left.write(out, depth);
                m.right.write(out, depth);
            }
            Plan::SemiJoin(s) => {
                s.left.write(out, depth);
                s.right.write(out, depth);
            }
            Plan::RangeJoin(r) => {
                r.left.write(out, depth);
                r.right.write(out, depth);
            }
        }
    }
}

/// The word EXPLAIN shows after Aggregate or Distinct if it is streamed,
/// its input in its keys' order (RULES.md D550).
fn streamed_word(streamed: bool) -> &'static str {
    if streamed { " streamed" } else { "" }
}

/// How EXPLAIN shows a scan in the order of the columns `order`, read
/// `backward` if so: nothing, of none.
fn scan_order(order: &[usize], backward: bool) -> String {
    match (order.is_empty(), backward) {
        (true, _) => String::new(),
        (false, false) => format!(" in order of {}", order.len()),
        (false, true) => format!(" in order of {} backward", order.len()),
    }
}

/// What EXPLAIN shows of the unnesting `u`: a GRAPH_TABLE's searches and
/// automata, a JSON table, or the collections unnested.
fn write_unnest(u: &Unnest<Plan>, out: &mut String, pad: &str) -> std::fmt::Result {
    match (&u.graph, &u.json) {
        // [GS §16 R16.9] a GRAPH_TABLE: its searches and automata.
        (Some(g), _) => {
            let left = if u.outer.is_some() { "LeftOuter " } else { "" };
            let lines: Vec<String> = g.render().lines().map(|l| format!("{pad}{left}{l}")).collect();
            writeln!(out, "{}", lines.join("\n"))
        }
        // [ISO/IEC 9075-2:2023 §7.11] a JSON table.
        (None, Some(j)) => writeln!(out, "{pad}{}{}", if u.outer.is_some() { "LeftOuter " } else { "" }, j.render()),
        (None, None) => writeln!(
            out,
            "{pad}{}Unnest of {} collections{}",
            if u.outer.is_some() { "LeftOuter" } else { "" },
            u.collections.len(),
            if u.ordinality { " with ordinality" } else { "" }
        ),
    }
}

/// The line of a nearest read: its index, measure and cost, and how it
/// reads, certified or approximate (R13.5).
fn nearest_line(read: &super::NearestRead) -> String {
    let how = read.approximate.as_ref().map_or_else(|| "certified bounds or scan".to_owned(), super::ApproximateRead::render);
    format!(
        "NearestScan {} using {} [{}]: {}  (first={}, cost={:.1}; {how})",
        read.table, read.index, read.method, read.detail, read.first, read.cost
    )
}

/// \[EXTENSION\] (RULES.md D376) Writes the counted operator `c`, `depth`
/// levels below the top, its line followed by what it did: its rows, the
/// times it was opened if not once, an index join's probes, which are its
/// outer rows, and its time if its clock moved; and its estimate before
/// them, if its line shows none.
fn write_counted(c: &super::Counted, out: &mut String, depth: usize) {
    let mut text = String::new();
    c.input.write(&mut text, depth);
    let (head, rest) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    // Writing to a String cannot fail.
    let _ = writeln!(out, "{head}{}", counted_note(c));
    out.push_str(rest);
}

/// What EXPLAIN ANALYZE adds to the line of the counted operator `c`.
fn counted_note(c: &super::Counted) -> String {
    let est = match (c.input.shows_estimate(), c.input.est()) {
        (false, Some(est)) => format!("  (est. rows={est:.0})"),
        (true, _) | (false, None) => String::new(),
    };
    let count = &c.count;
    if count.opens() == 0 {
        return format!("{est}  (never run)");
    }
    let mut actual = format!("rows={}", count.rows());
    if count.opens() > 1 {
        let _ = write!(actual, ", loops={}", count.opens());
    }
    if let Plan::IndexJoin { left, .. } = &c.input
        && let Plan::Counted(outer) = &**left
    {
        let _ = write!(actual, ", probes={}", outer.count.rows());
    }
    if let Some(micros) = count.micros() {
        let _ = write!(actual, ", time={}", millis(micros));
    }
    noted_of(&count.noted(), count.rows(), &mut actual);
    format!("{est}  (actual {actual})")
}

/// \[EXTENSION\] What `notes` hold of an operator that gave `rows` rows,
/// added to `actual` (RULES.md D620, D621): its batches and their mean
/// rows, what it spilled, and each count of what it read of indexes, each
/// only if any.
fn noted_of(notes: &NotesRead, rows: u64, actual: &mut String) {
    // Writing to a String cannot fail.
    if let Some(mean) = rows.checked_add(notes.batches.checked_div(2).unwrap_or_default()).and_then(|r| r.checked_div(notes.batches)) {
        let _ = write!(actual, ", batches={}, mean rows={mean}", notes.batches);
    }
    if notes.partitions > 0 {
        let _ = write!(actual, ", spilled={} bytes, partitions={}, passes={}", notes.spilled, notes.partitions, notes.passes);
    }
    let read = [("entries", notes.entries), ("leaf pages", notes.leaves), ("fetched", notes.fetched), ("rechecked", notes.rechecked)];
    for (name, n) in read.into_iter().filter(|&(_, n)| n > 0) {
        let _ = write!(actual, ", {name}={n}");
    }
}

/// `micros` microseconds, as milliseconds to the microsecond.
pub(crate) fn millis(micros: u64) -> String {
    const PER_MILLI: u64 = 1_000;
    format!("{}.{:03} ms", micros.checked_div(PER_MILLI).unwrap_or_default(), micros.checked_rem(PER_MILLI).unwrap_or_default())
}

impl Plan {
    /// Whether its own line shows its estimate: a table's read's, an
    /// index join's, a filter's of a table's read.
    fn shows_estimate(&self) -> bool {
        matches!(self, Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::IndexJoin { .. } | Plan::Filter { of_read: true, .. })
    }
}

// Unit tests: a plan shown is described, one run is not; what a counted
// operator adds to its line (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] an operator is described by its input's column
    /// names when the plan is shown, and by nothing when it is run.
    #[test]
    fn an_operator_is_described_only_when_the_plan_is_shown() {
        let names = ["A".to_string(), "B".to_string()];
        assert_eq!(Shown::of(Some(&names)).text(|n| n.join(", ")), "A, B");
        assert_eq!(Shown::of(None).text(|n| n.join(", ")), "");
        assert_eq!(Shown::of(Some(&names)).names().map(<[String]>::len), Some(2));
        assert!(Shown::of(None).names().is_none());
    }

    /// The counted list of one row of one value, its line `Values 1`.
    fn counted_values() -> super::super::Counted {
        let input = Plan::Values { rows: vec![vec![crate::logical::Expr::Const(crate::value::Value::Null)]], detail: "1".into() };
        super::super::Counted { count: std::sync::Arc::default(), input }
    }

    /// Verifies: [EXTENSION] RULES.md D376, D378, R-TEST-8: a counted
    /// operator's line is followed by its estimate, as its line shows none,
    /// then never run, if it was not opened; else its rows, its openings
    /// past one, and its time once its clock moved, in milliseconds to the
    /// microsecond.
    #[test]
    fn a_counted_operator_shows_what_it_did() {
        let c = counted_values();
        let shown = || Plan::Counted(Box::new(c.clone())).explain();
        assert_eq!(shown(), "Values 1  (est. rows=1)  (never run)\n");
        c.count.opened();
        c.count.add(1, 1_234);
        assert_eq!(shown(), "Values 1  (est. rows=1)  (actual rows=1)\n");
        c.count.opened();
        c.count.show_time(true);
        assert_eq!(shown(), "Values 1  (est. rows=1)  (actual rows=1, loops=2, time=1.234 ms)\n");
        c.count.stepped(2);
        assert_eq!(shown(), "Values 1  (est. rows=1)  (actual rows=3, loops=2)\n", "a step has no time of its own");
        assert_eq!(millis(5), "0.005 ms");
    }
}
