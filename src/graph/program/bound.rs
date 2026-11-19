//! \[DESIGN\] What a prefilter is given [GS §6.8]: the element it tests,
//! the bindings of the variables it reads, singletons and groups [P1
//! §4.4], PREV and FIRST [GS §11.1], and the path matched so far.
//!
//! The path so far is a register of the thread's, of its elements in
//! order and a sign of each subpath mark where it is bound, so a
//! prefilter's path function sees the partial path, as a thread's other
//! registers tell its bindings apart (RULES.md D142).

use super::VarId;
use crate::graph::binding::PathBinding;
use crate::graph::view::{ElementId, TableId};

/// The variable a read of the whole path names, as a subpath's marks.
pub const WHOLE_PATH: VarId = VarId::MAX;

/// The table of no element: of a mark's sign in the path register, and
/// of a binding not made in a deferred prefilter's snapshot.
const NO_TABLE: TableId = TableId::MAX;

/// The sign in the path register of mark `var` bound.
pub(in crate::graph) fn marked(var: VarId) -> ElementId {
    ElementId { table: NO_TABLE, row: u64::from(var) }
}

/// The sign of no binding, in a deferred prefilter's snapshot.
pub(in crate::graph) fn absent() -> ElementId {
    ElementId { table: NO_TABLE, row: u64::MAX }
}

/// Whether `element` is a sign rather than an element.
pub(in crate::graph) fn is_absent(element: ElementId) -> bool {
    element.table == NO_TABLE
}

/// A variable a prefilter reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    /// The variable; of a path read, the mark where its subpath begins,
    /// or `WHOLE_PATH`.
    pub var: VarId,
    /// Whether it reads `PREV(var)` [GS §11.1] rather than its binding.
    pub previous: bool,
    /// Whether it reads `FIRST(var)` [GS §11.1] rather than its binding.
    pub first: bool,
    /// Whether it is bound after the element pattern whose prefilter
    /// reads it, which is then tested as it is bound.
    pub later: bool,
    /// Of a path function's read of the path so far, the mark where its
    /// subpath ends, or `WHOLE_PATH`.
    pub path: Option<VarId>,
}

impl Read {
    /// A read of `var`'s binding.
    pub fn of(var: VarId) -> Read {
        Read { var, previous: false, first: false, later: false, path: None }
    }
}

/// What a prefilter is given: the element it tests, if it is an element
/// pattern's, the bindings of the variables it reads, and the path so far.
pub struct Bound<'a> {
    pub(in crate::graph) element: Option<(Option<VarId>, ElementId)>,
    pub(in crate::graph) registers: &'a [Box<[ElementId]>],
    /// For each variable read, its register, its PREV register and its
    /// FIRST register.
    pub(in crate::graph) reads: &'a [Resolved],
    /// The path register: the path's elements so far and its marks.
    pub(in crate::graph) path: &'a [ElementId],
}

/// A variable a prefilter reads, resolved: its register, its PREV
/// register and its FIRST register, each if it reads it so.
pub(in crate::graph) type Resolved = (VarId, Option<usize>, Option<usize>, Option<usize>);

impl Bound<'static> {
    /// What the prefilter of an element pattern of variable `var` that
    /// reads no other variable is given of `element` alone, as a search
    /// that has not reached it asks beforehand (RULES.md D150).
    pub fn alone(var: Option<VarId>, element: ElementId) -> Bound<'static> {
        Bound { element: Some((var, element)), registers: &[], reads: &[], path: &[] }
    }
}

impl Bound<'_> {
    /// The element tested, for an element pattern's prefilter.
    pub fn element(&self) -> Option<ElementId> {
        self.element.map(|(_, e)| e)
    }

    /// The elements `var` is bound to so far, within the prefilter's
    /// pattern: one for a singleton, a list for a group [P1 §4.4].
    pub fn group(&self, var: VarId) -> &[ElementId] {
        let register = self.reads.iter().find_map(|r| r.1.filter(|_| r.0 == var && r.3.is_none()));
        let held = register.and_then(|r| self.registers.get(r)).map_or(&[][..], |elements| &**elements);
        if held.iter().copied().any(is_absent) { &[] } else { held }
    }

    /// The element singleton `var` is bound to, if it is bound yet: the
    /// element tested, if it is the element pattern's own variable.
    pub fn singleton(&self, var: VarId) -> Option<ElementId> {
        let own = self.element.and_then(|(own, element)| (own == Some(var)).then_some(element));
        own.or_else(|| self.group(var).last().copied())
    }

    /// The last element of register `register`, if any.
    fn last(&self, register: Option<usize>) -> Option<ElementId> {
        register.and_then(|r| self.registers.get(r)).and_then(|elements| elements.last().copied()).filter(|e| !is_absent(*e))
    }

    /// \[GS §11.1\] `PREV(var)`: its binding in the previous iteration of
    /// the quantifier the prefilter is within, none in the first.
    pub fn previous(&self, var: VarId) -> Option<ElementId> {
        let register = self.reads.iter().find_map(|r| r.2.filter(|_| r.0 == var))?;
        self.last(Some(register))
    }

    /// \[GS §11.1\] `FIRST(var)`: its binding in the first iteration of
    /// the quantifier the prefilter is within; in the first, its binding
    /// in this one.
    pub fn first(&self, var: VarId) -> Option<ElementId> {
        let read = self.reads.iter().find(|r| r.0 == var && r.3.is_some())?;
        let own = self.element.and_then(|(own, element)| (own == Some(var)).then_some(element));
        self.last(read.3).or_else(|| self.last(read.1)).or(own)
    }

    /// \[GS §6.9\] The path matched so far, the element tested its last;
    /// of `span`, the marks of a subpath variable, the part of it from
    /// where the first is bound to where the second is, none if the
    /// subpath has not begun.
    pub fn partial(&self, span: Option<(VarId, VarId)>) -> Option<PathBinding> {
        let (mut elements, mut on, mut last) = (Vec::new(), span.is_none(), None);
        for &e in self.path.iter().chain(self.element().as_ref()) {
            if !is_absent(e) {
                last = Some(e);
                if on && elements.last() != Some(&e) {
                    elements.push(e);
                }
            } else if span.is_some_and(|(start, _)| e == marked(start)) {
                on = true;
                elements = last.into_iter().collect();
            } else if on && span.is_some_and(|(_, end)| e == marked(end)) {
                break;
            }
        }
        on.then(|| {
            let vars = vec![Vec::new(); elements.len()];
            let tags = vec![Vec::new(); elements.len().div_ceil(2)];
            PathBinding { elements, vars, tags, cost: None }
        })
    }
}
