//! \[EXTENSION\] RULES.md D106: a lookup as EXPLAIN shows it, written as
//! the path and comparisons it selects by: `/` before a child step, `//`
//! before a descendant step, `@` before an attribute's name, `*` for any
//! name, a name in a namespace as `Q{uri}local` [XPath 3.0 §A.1 EQName],
//! then each comparison with its literal; a query's filter of an ancestor
//! as a predicate of the step it is of, its path relative to it, `.` the
//! item [XQuery §3.2.2].

use std::fmt::{self, Display, Write};

use super::lookup::{Axis, Comparison, Literal, PathPattern, Step, Target, XmlLookup, XmlQuery};
use super::probe::XmlProbe;
use crate::value::CompOp;

impl Display for XmlLookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        steps(f, &self.pattern, false)?;
        compared(f, &self.comparisons)
    }
}

impl Display for XmlQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(filter) = &self.filter else { return self.lookup.fmt(f) };
        let pattern = &self.lookup.pattern;
        let anchor = pattern.steps.len().saturating_sub(filter.above);
        let (above, below) = pattern.steps.split_at(anchor);
        if above.is_empty() {
            f.write_char('.')?;
        }
        steps(f, &PathPattern { steps: above.to_vec(), target: Target::Element }, false)?;
        f.write_char('[')?;
        steps(f, &filter.pattern, true)?;
        compared(f, &filter.comparisons)?;
        f.write_char(']')?;
        steps(f, &PathPattern { steps: below.to_vec(), target: pattern.target }, false)?;
        compared(f, &self.lookup.comparisons)
    }
}

impl Display for XmlProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.query.fmt(f)?;
        if self.answered { f.write_str(", answered by the index") } else { Ok(()) }
    }
}

/// The steps of `pattern`, a path `relative` to a node if so: its first
/// step written without `/`, or `.//` of a descendant.
fn steps(f: &mut fmt::Formatter<'_>, pattern: &PathPattern, relative: bool) -> fmt::Result {
    let last = pattern.steps.len().saturating_sub(1);
    for (at, step) in pattern.steps.iter().enumerate() {
        let before = match (step.axis, relative && at == 0) {
            (Axis::Child, false) => "/",
            (Axis::Child, true) => "",
            (Axis::Descendant, false) => "//",
            (Axis::Descendant, true) => ".//",
        };
        f.write_str(before)?;
        if at == last && pattern.target == Target::Attribute {
            f.write_char('@')?;
        }
        name(f, step)?;
    }
    Ok(())
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
