//! \[EXTENSION\] INDEX_ACCURACY_REPORT (the vector design document §10.6):
//! an approximate index's recall, measured against exact search. Each
//! sample query vector is searched both ways for its `k` nearest rows by
//! the index's metric, as FETCH APPROX and FETCH EXACT search, and the
//! rows found both ways are counted. A Rust call, not a statement, as a
//! statement gives no rows.

use std::collections::HashSet;

use crate::db::{Cursor, Session};
use crate::error::{Code, Result, SqlError};
use crate::value::Value;

/// The distance function a report searches by.
const DISTANCE: &str = crate::vector::functions::VECTOR_DISTANCE;

/// What an accuracy report measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccuracyReport {
    /// The sample queries searched.
    pub queries: usize,
    /// The rows exact search found, of every query.
    pub expected: usize,
    /// Of those, the rows the approximate search found too.
    pub found: usize,
}

impl AccuracyReport {
    /// The recall, the share of the rows exact search found that the
    /// approximate search found too, 1 of none.
    #[must_use]
    pub fn recall(&self) -> f64 {
        let (Ok(found), Ok(expected)) = (u32::try_from(self.found), u32::try_from(self.expected)) else { return 0.0 };
        if expected == 0 { 1.0 } else { f64::from(found) / f64::from(expected) }
    }
}

impl Session {
    /// \[EXTENSION\] INDEX_ACCURACY_REPORT(`index`, `samples`, `k`): the
    /// recall of the index, each of `samples` searched for its `k` nearest
    /// rows by `VECTOR_DISTANCE` of the index's metric, approximately and
    /// exactly, in this session's transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no index `index`, or it has no metric (it
    /// is not approximate); any error a search raises.
    pub fn index_accuracy_report(&self, index: &str, samples: &[Value], k: usize) -> Result<AccuracyReport> {
        let (head, tail) = self.reported(index)?;
        let mut report = AccuracyReport { queries: 0, expected: 0, found: 0 };
        for sample in samples {
            // Each sample a literal, so each search is planned for its value.
            let sql = format!("{head}{}{tail}", sample.to_literal());
            let found: HashSet<String> = keys(self.open_cursor(&format!("{sql} FETCH APPROX FIRST {k} ROWS ONLY"))?)?.into_iter().collect();
            let expected = keys(self.open_cursor(&format!("{sql} FETCH EXACT FIRST {k} ROWS ONLY"))?)?;
            let both = expected.iter().filter(|r| found.contains(*r)).count();
            report.queries = report.queries.saturating_add(1);
            report.expected = report.expected.saturating_add(expected.len());
            report.found = report.found.saturating_add(both);
        }
        Ok(report)
    }

    /// The query of a report of `index`, up to its fetch clause, before
    /// and after the sample's place.
    fn reported(&self, index: &str) -> Result<(String, String)> {
        let st = self.db.snapshot()?;
        let refused = |why: String| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {why}"));
        let ix = st.catalog.index(index).ok_or_else(|| refused(format!("there is no index {index}")))?;
        let method = st.methods.get(&ix.method).ok_or_else(|| refused(format!("there is no index method {}", ix.method)))?;
        let metric = method.metric(ix).ok_or_else(|| refused(format!("the index {index} is not approximate")))?;
        let table = st.catalog.table(&ix.table).ok_or_else(|| refused(format!("there is no table {}", ix.table)))?;
        let column = ix.columns.first().and_then(|c| table.columns.get(*c)).ok_or_else(|| refused(format!("{index} has no column")))?;
        let quoted = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        // Each column named, as a nearest read is planned of one projection.
        let all: Vec<String> = table.columns.iter().map(|c| quoted(&c.name)).collect();
        let (schema, name, column) = (quoted(&ix.table.auth), quoted(&ix.table.ident), quoted(&column.name));
        let head = format!("SELECT {} FROM {schema}.{name} ORDER BY {DISTANCE}({column}, ", all.join(", "));
        Ok((head, format!(", '{metric}')")))
    }
}

/// Each row of `cursor`, as text.
fn keys(mut cursor: Cursor) -> Result<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = cursor.fetch()? {
        rows.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(rows)
}
