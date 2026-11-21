//! \[DESIGN\] Mark joins planned (RULES.md D642): each row of the left
//! operand, in its order, with its mark, true, false or unknown
//! (`logical/mark.rs`).
//!
//! * A hash mark join builds the right's rows by the equalities of a column
//!   of each operand that its condition holds, its keys, and, when the
//!   comparison it marks by is an AND of such equalities, as of IN, by
//!   those too, its marks. A right row of a null mark value is kept apart
//!   by its keys, on the null list of Neumann, Leis and Kemper (BTW 2017,
//!   §5.6): it makes the comparison unknown, never false, for every left
//!   row of its keys (`exec/join/mark.rs`).
//! * A nested-loop mark join, of a condition of no equality, holds the
//!   right's rows and reads them for each left row until one makes the
//!   mark true.
//!
//! The condition beyond the keys, and the comparison, are evaluated only
//! on the pairs the keys keep, and of those only until one is true, so the
//! optimizer makes mark joins only of conditions that cannot raise (D15).

use super::built::Built;
use super::join::{JoinKey, equated, keys_detail};
use super::shown::Shown;
use super::{PhysicalPlanner, Plan, of_left};
use crate::logical::{Cond, MarkJoin as Logical, SortKey, and_all, judged_by_sets, render_cond, split_and};

/// \[DESIGN\] How a mark join is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkMethod {
    /// The right built by its keys and marks, each left row probing it.
    Hash,
    /// The right held, each left row read against its rows.
    Nested,
}

/// \[DESIGN\] A mark join (`LogicalPlan::MarkJoin`).
#[derive(Debug, Clone)]
pub struct MarkJoin {
    /// How it is made.
    pub method: MarkMethod,
    /// The rows marked.
    pub left: Plan,
    /// The rows matched against.
    pub right: Plan,
    /// Pairs of a column of `left` and one of `right` whose values a right
    /// row must match to count.
    pub keys: Vec<JoinKey>,
    /// The rest of the condition a right row must make true to count, over
    /// the pair's columns.
    pub cond: Option<Cond>,
    /// Pairs of a column of each whose equalities' AND is `compared`, or
    /// none.
    pub marks: Vec<JoinKey>,
    /// The comparison whose truth values the mark ORs, over the pair's
    /// columns; none of EXISTS.
    pub compared: Option<Cond>,
    /// The degrees of `left` and `right`.
    pub widths: (usize, usize),
    /// The conditions, as text.
    pub detail: String,
    /// Its estimated rows, its left's.
    pub est: f64,
}

impl MarkJoin {
    /// The name EXPLAIN gives it.
    pub fn operator(&self) -> &'static str {
        match self.method {
            MarkMethod::Hash => "HashMarkJoin",
            MarkMethod::Nested => "NestedLoopMarkJoin",
        }
    }
}

/// The equalities of a column of each operand among the conjuncts of
/// `cond`, of a left `width` columns wide, and the other conjuncts.
fn keyed(cond: &Cond, width: usize) -> (Vec<JoinKey>, Vec<Cond>) {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut keys, mut rest) = (Vec::new(), Vec::new());
    for c in conjuncts {
        match equated(&c, width) {
            Some(key) => keys.push(key),
            None => rest.push(c),
        }
    }
    (keys, rest)
}

/// The columns of the equalities whose AND `compared` is, of a left
/// `width` columns wide, or none if it is not such an AND.
fn marked(compared: Option<&Cond>, width: usize) -> Vec<JoinKey> {
    let Some(c) = compared else { return Vec::new() };
    let (marks, rest) = keyed(c, width);
    // A key on which nulls match is of IS NOT DISTINCT FROM, never true
    // and false as a comparison is.
    if rest.is_empty() && marks.iter().all(|k| !k.nulls) { marks } else { Vec::new() }
}

impl PhysicalPlanner<'_> {
    /// The mark join `m`, its rows in its left's order.
    pub(super) fn mark_join(&self, m: &Logical, want: &[SortKey]) -> Built {
        let width = m.left.width();
        let est = self.rows_of(&m.left);
        let want_left = of_left(want, width);
        let (l, r) = (self.build_node(&m.left, want_left), self.build_node(&m.right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        let (keys, rest) = keyed(&m.cond, width);
        let marks = marked(m.compared.as_ref(), width);
        let method = if keys.is_empty() && marks.is_empty() { MarkMethod::Nested } else { MarkMethod::Hash };
        let both = self.shown.then(|| m.left.names().into_iter().chain(m.right.names()).collect::<Vec<String>>());
        let shown = Shown::of(both.as_deref());
        let detail = shown.text(|names| {
            let mut text = render_cond(&m.cond, names);
            if let Some(c) = &m.compared {
                text = format!("{text}  (marking {})", render_cond(c, names));
            }
            let hashed: Vec<JoinKey> = keys.iter().chain(&marks).copied().collect();
            if hashed.is_empty() { text } else { format!("{text}  (hashed on {})", keys_detail(&hashed, names, width)) }
        });
        let cond = and_all(rest).map(judged_by_sets);
        let compared = m.compared.clone().map(judged_by_sets);
        let widths = (width, m.right.width());
        let join = MarkJoin { method, left: l.plan, right: r.plan, keys, cond, marks, compared, widths, detail, est };
        Built { plan: Plan::MarkJoin(Box::new(join)), ordered }
    }
}
