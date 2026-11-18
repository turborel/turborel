//! \[EXTENSION\] RULES.md D86: a lookup as EXPLAIN shows it, written as
//! the path and comparisons it selects by: `/` before a child step, `//`
//! before a descendant step, `@` before an attribute's name, `*` for any
//! name, a name in a namespace as `Q{uri}local` [XPath 3.0 §A.1 EQName],
//! then each comparison with its literal.

use std::fmt::{self, Display, Write};

use super::lookup::{Axis, Literal, Target, XmlLookup};
use crate::value::CompOp;

impl Display for XmlLookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pattern = &self.pattern;
        let last = pattern.steps.len().saturating_sub(1);
        for (at, step) in pattern.steps.iter().enumerate() {
            let before = match step.axis {
                Axis::Child => "/",
                Axis::Descendant => "//",
            };
            f.write_str(before)?;
            if at == last && pattern.target == Target::Attribute {
                f.write_char('@')?;
            }
            match &step.name {
                None => f.write_char('*')?,
                Some(name) => match &name.uri {
                    Some(uri) => write!(f, "Q{{{uri}}}{}", name.local)?,
                    None => f.write_str(&name.local)?,
                },
            }
        }
        for comparison in &self.comparisons {
            write!(f, " {} {}", operator(comparison.op), Shown(&comparison.literal))?;
        }
        Ok(())
    }
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
