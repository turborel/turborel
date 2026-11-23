//! \[DESIGN\] A leaf of a program evaluated where it is (RULES.md D610):
//! the frame's values read as its slots, and the leaf's outer values as
//! its outer references, so neither is copied into its expressions.

use std::sync::Arc;

use crate::catalog::SequenceDesc;
use crate::db::State;
use crate::error::{Result, Warnings};
use crate::exec::{Subqueries, SubqueryRows};
use crate::fulltext::rank::Statistics;
use crate::logical::{Invocation, Subquery};
use crate::value::Value;

/// The statement's subqueries, `inner`, with the frame's values `frame`
/// and the leaf's outer values `outer`.
pub(crate) struct Framed<'a> {
    pub(crate) inner: &'a dyn Subqueries,
    pub(crate) frame: &'a [Value],
    pub(crate) outer: &'a [Value],
}

impl Subqueries for Framed<'_> {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        self.inner.rows(subquery, outer, limit)
    }

    fn slot(&self, index: usize) -> Option<&Value> {
        self.frame.get(index)
    }

    fn outer(&self, index: usize) -> Option<&Value> {
        self.outer.get(index)
    }

    fn warnings(&self) -> Option<&Warnings> {
        self.inner.warnings()
    }

    fn like(&self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        self.inner.like(value, pattern, escape)
    }

    fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        self.inner.next_value(seq)
    }

    fn interrupted(&self) -> Result<()> {
        self.inner.interrupted()
    }

    fn state(&self) -> Option<&Arc<State>> {
        self.inner.state()
    }

    fn ranking_statistics(&self, index: &str) -> Result<Option<Arc<dyn Statistics>>> {
        self.inner.ranking_statistics(index)
    }

    fn temp_lobs(&self) -> Option<&dyn crate::value::lob::TempLobs> {
        self.inner.temp_lobs()
    }

    fn invoke_hosted(&self, i: &Invocation, values: Vec<Value>) -> Option<Result<Option<Value>>> {
        self.inner.invoke_hosted(i, values)
    }
}
