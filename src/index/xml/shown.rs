//! \[EXTENSION\] RULES.md D106: a lookup as EXPLAIN shows it, written as
//! the path and comparisons it selects by: `/` before a child step, `//`
//! before a descendant step, `@` before an attribute's name, `*` for any
//! name, a name in a namespace as `Q{uri}local` [XPath 3.0 §A.1 EQName],
//! then each comparison with its literal; a query's filters as predicates
//! of the steps they are of, their paths relative to them, `.` the item
//! [XQuery §3.2.2]; a relation as the last step it is, its node's
//! comparisons a predicate of `.` before it; a probe's queries joined by
//! `and`.

use std::fmt::{self, Display, Write};

use super::lookup::{Axis, Comparison, Literal, PathPattern, Step, Target, XmlFilter, XmlLookup, XmlQuery};
use super::probe::XmlProbe;
use super::related::Relation;
use crate::value::CompOp;

impl Display for XmlLookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        steps(f, &self.pattern, false)?;
        compared(f, &self.comparisons)
    }
}

impl Display for XmlQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let of = |at: usize| self.filters.iter().filter(move |filter| filter.at == at);
        if of(0).next().is_some() {
            f.write_char('.')?;
        }
        predicates(f, of(0))?;
        let pattern = &self.lookup.pattern;
        let last = pattern.steps.len().saturating_sub(1);
        for (at, each) in pattern.steps.iter().enumerate() {
            step(f, each, (at == last && pattern.target == Target::Attribute, false))?;
            predicates(f, of(at.saturating_add(1)))?;
        }
        let Some(related) = &self.related else { return compared(f, &self.lookup.comparisons) };
        if !self.lookup.comparisons.is_empty() {
            f.write_str("[.")?;
            compared(f, &self.lookup.comparisons)?;
            f.write_char(']')?;
        }
        write!(f, "/{}::", axis(related.relation))?;
        name(f, &Step { axis: Axis::Child, name: related.name.clone() })
    }
}

/// The axis of `relation`, as XQuery names it [XQuery §3.2.1.1].
fn axis(relation: Relation) -> &'static str {
    match relation {
        Relation::Ancestor => "ancestor",
        Relation::AncestorOrSelf => "ancestor-or-self",
        Relation::Following => "following",
        Relation::FollowingSibling => "following-sibling",
        Relation::Preceding => "preceding",
        Relation::PrecedingSibling => "preceding-sibling",
    }
}

impl Display for XmlProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, query) in self.queries.iter().enumerate() {
            if n > 0 {
                f.write_str(" and ")?;
            }
            query.fmt(f)?;
        }
        if self.answered { f.write_str(", answered by the index") } else { Ok(()) }
    }
}

/// Each of `filters` as a predicate, its path relative to its anchor.
fn predicates<'f>(f: &mut fmt::Formatter<'_>, filters: impl Iterator<Item = &'f XmlFilter>) -> fmt::Result {
    for filter in filters {
        f.write_char('[')?;
        steps(f, &filter.pattern, true)?;
        compared(f, &filter.comparisons)?;
        f.write_char(']')?;
    }
    Ok(())
}

/// The steps of `pattern`, a path `relative` to a node if so: its first
/// step written without `/`, or `.//` of a descendant.
fn steps(f: &mut fmt::Formatter<'_>, pattern: &PathPattern, relative: bool) -> fmt::Result {
    let last = pattern.steps.len().saturating_sub(1);
    for (at, each) in pattern.steps.iter().enumerate() {
        step(f, each, (at == last && pattern.target == Target::Attribute, relative && at == 0))?;
    }
    Ok(())
}

/// `each`, of an attribute if `attribute`, the first of a relative path if
/// `first`.
fn step(f: &mut fmt::Formatter<'_>, each: &Step, (attribute, first): (bool, bool)) -> fmt::Result {
    let before = match (each.axis, first) {
        (Axis::Child, false) => "/",
        (Axis::Child, true) => "",
        (Axis::Descendant, false) => "//",
        (Axis::Descendant, true) => ".//",
    };
    f.write_str(before)?;
    if attribute {
        f.write_char('@')?;
    }
    name(f, each)
}

/// The name a step tests, `*` of any.
fn name(f: &mut fmt::Formatter<'_>, step: &Step) -> fmt::Result {
    match &step.name {
        None => f.write_char('*'),
        Some(name) => match &name.uri {
            Some(uri) => write!(f, "Q{{{uri}}}{}", name.local),
            None => f.write_str(&name.local),
        },
    }
}

/// Each of `comparisons`, its operator and its literal.
fn compared(f: &mut fmt::Formatter<'_>, comparisons: &[Comparison]) -> fmt::Result {
    for comparison in comparisons {
        write!(f, " {} {}", operator(comparison.op), Shown(&comparison.literal))?;
    }
    Ok(())
}

/// The general comparison operator of `op` [XQuery §3.5.2].
fn operator(op: CompOp) -> &'static str {
    match op {
        CompOp::Eq => "=",
        CompOp::Ne => "!=",
        CompOp::Lt => "<",
        CompOp::Le => "<=",
        CompOp::Gt => ">",
        CompOp::Ge => ">=",
    }
}

/// A literal as XQuery writes it: a string quoted, its quotes doubled.
struct Shown<'a>(&'a Literal);

impl Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Literal::String(s) | Literal::Untyped(s) => write!(f, "'{}'", s.replace('\'', "''")),
            Literal::Number(n) => write!(f, "{n}"),
        }
    }
}
