//! [ISO/IEC 9075-2:2023 §4.29.1] The values one row draws: every NEXT VALUE
//! FOR of one sequence generator in a statement gives the same value for a
//! row it processes, so each row's first draw of a generator is kept for
//! the rest of that row.

use std::cell::RefCell;

use super::{Subqueries, SubqueryRows};
use crate::catalog::{QualifiedName, SequenceDesc};
use crate::error::{Result, Warnings};
use crate::logical::Subquery;
use crate::value::Value;

/// \[DESIGN\] The subqueries of `inner`, and the values one row drew of
/// each sequence generator: one made for each row.
pub struct Drawn<'s> {
    inner: &'s dyn Subqueries,
    drawn: RefCell<Vec<(QualifiedName, u64, Value)>>,
}

impl<'s> Drawn<'s> {
    /// What a row draws, running its subqueries by `inner`.
    pub fn new(inner: &'s dyn Subqueries) -> Drawn<'s> {
        Drawn { inner, drawn: RefCell::new(Vec::new()) }
    }
}

impl Subqueries for Drawn<'_> {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        self.inner.rows(subquery, outer, limit)
    }

    fn warnings(&self) -> Option<&Warnings> {
        self.inner.warnings()
    }

    fn like(&self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        self.inner.like(value, pattern, escape)
    }

    /// The value the row drew of `seq` already, else one drawn now.
    fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        let held = self.drawn.borrow().iter().find(|(name, epoch, _)| *name == seq.name && *epoch == seq.epoch).map(|(_, _, v)| v.clone());
        if let Some(v) = held {
            return Ok(v);
        }
        let v = self.inner.next_value(seq)?;
        self.drawn.borrow_mut().push((seq.name.clone(), seq.epoch, v.clone()));
        Ok(v)
    }
}
