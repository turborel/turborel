//! \[DESIGN\] What EXPLAIN shows of an operator: text made of the names of
//! its input's columns, when a plan is made to be shown; none, when it is
//! made to run, so a statement run pays nothing for it; and a plan written
//! as EXPLAIN writes it, an operator a line.

use std::fmt::Write;

use super::{PhysicalPlanner, Plan};
use crate::logical::LogicalPlan;

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
            Plan::Filter { detail, est: None, .. } => writeln!(out, "{pad}Filter {detail}"),
            Plan::Filter { detail, est: Some(est), .. } => writeln!(out, "{pad}Filter {detail}  (est. rows={est:.0})"),
            Plan::Product { .. } => writeln!(out, "{pad}NestedLoop (Cartesian product)"),
            Plan::HashJoin { detail, .. } => writeln!(out, "{pad}HashJoin {detail}"),
            Plan::OuterJoin { kind, detail, .. } => writeln!(out, "{pad}{kind:?}OuterJoin {detail}"),
            Plan::IndexJoin { outer, table, index, method, detail, est, cost, .. } => writeln!(
                out,
                "{pad}{}IndexNestedLoop {table} using {index} [{method}]: {detail}  (est. rows each={est:.0}, cost={cost:.1})",
                if *outer { "LeftOuter" } else { "" }
            ),
            Plan::Aggregate { detail, .. } => writeln!(out, "{pad}Aggregate {detail}"),
            Plan::Project { detail, .. } => writeln!(out, "{pad}Project {detail}"),
            Plan::Distinct { .. } => writeln!(out, "{pad}Distinct"),
            Plan::Append { .. } => writeln!(out, "{pad}Append"),
            Plan::SetOp { op, all, .. } => writeln!(out, "{pad}{op:?}{}", if *all { " all" } else { "" }),
            Plan::Values { detail, .. } => writeln!(out, "{pad}Values {detail}"),
            Plan::System { read } => writeln!(out, "{pad}System {}", read.name()),
            Plan::Sort { detail, .. } => writeln!(out, "{pad}Sort {detail}"),
            Plan::Recursive { member, fixpoint } => writeln!(out, "{pad}Recursive member {member} ({})", fixpoint.word()),
            Plan::WorkTable { member, .. } => writeln!(out, "{pad}WorkTable of member {member}"),
            Plan::Unnest(u) => writeln!(
                out,
                "{pad}{}Unnest of {} collections{}",
                if u.outer.is_some() { "LeftOuter" } else { "" },
                u.collections.len(),
                if u.ordinality { " with ordinality" } else { "" }
            ),
            Plan::Window { detail, .. } => writeln!(out, "{pad}{detail}"),
        };
        self.write_inputs(out, depth.saturating_add(1));
    }

    /// Writes its inputs, each at `depth`.
    fn write_inputs(&self, out: &mut String, depth: usize) {
        match self {
            Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::Values { .. } | Plan::System { .. } | Plan::WorkTable { .. } => {}
            Plan::Recursive { fixpoint, .. } => {
                let (first, second, rest) = fixpoint.plans();
                for p in first.into_iter().chain(second).chain(rest) {
                    p.write(out, depth);
                }
            }
            Plan::Product { left, right }
            | Plan::HashJoin { left, right, .. }
            | Plan::OuterJoin { left, right, .. }
            | Plan::Append { left, right }
            | Plan::SetOp { left, right, .. } => {
                left.write(out, depth);
                right.write(out, depth);
            }
            Plan::Filter { input, .. }
            | Plan::Aggregate { input, .. }
            | Plan::Project { input, .. }
            | Plan::Distinct { input }
            | Plan::Sort { input, .. }
            | Plan::IndexJoin { left: input, .. } => input.write(out, depth),
            Plan::Unnest(u) => u.input.write(out, depth),
            Plan::Window { window, .. } => window.input.write(out, depth),
        }
    }
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

// Unit tests: a plan shown is described, one run is not (RULES.md R-TEST-1).
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
}
