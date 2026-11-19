//! [ISO/IEC 9075-2:2023 §7.6] A `<data change delta table>`: the rows a
//! data change statement inserted, changed or deleted, read as a table by
//! the query that runs it (RULES.md D174).

use std::fmt;
use std::sync::{Arc, Mutex};

use super::{SqlStatement, TableName};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::exec::TransitionRows;

/// [ISO/IEC 9075-2:2023 §7.6] `<result option>` ::= FINAL | NEW | OLD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaResult {
    /// OLD: the rows as they were before the statement.
    Old,
    /// NEW: the rows as the statement left them, before its AFTER triggers.
    New,
    /// FINAL: as NEW, and no AFTER trigger nor referential action of the
    /// statement may change its subject table \[GR5a\].
    Final,
}

/// [ISO/IEC 9075-2:2023 §7.6] `<data change delta table>` ::= `<result
/// option>` TABLE ( `<data change statement>` ), with its correlation name.
#[derive(Debug, Clone)]
pub struct DeltaTable {
    /// The `<result option>`.
    pub result: DeltaResult,
    /// The `<data change statement>`: INSERT, UPDATE, DELETE or MERGE.
    pub statement: Box<SqlStatement>,
    /// The `<correlation name>`, if written.
    pub correlation: Option<String>,
    /// The `<derived column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// Where its `<result option>` is written.
    pub at: usize,
    /// Its rows, once its statement ran.
    pub rows: DeltaRows,
}

impl DeltaTable {
    /// The subject table of its statement, if it is a `<data change
    /// statement>` [§7.6 SR11].
    pub fn subject(&self) -> Option<&TableName> {
        let statement = &*self.statement;
        if let SqlStatement::Insert(i) = statement {
            return Some(&i.table);
        }
        if let SqlStatement::UpdateSearched { table, .. } | SqlStatement::DeleteSearched { table, .. } = statement {
            return Some(table);
        }
        if let SqlStatement::Merge(m) = statement { Some(&m.target) } else { None }
    }
}

/// \[DESIGN\] The rows of a `<data change delta table>`, given once its
/// statement ran, with the subject table's name and columns, and whether
/// it stands where its statement is run (RULES.md D174): as the one table
/// reference of the FROM clause of a cursor's query. Elsewhere it is
/// refused as it is bound.
#[derive(Clone, Default)]
pub struct DeltaRows(Arc<Mutex<Held>>);

/// What a `DeltaRows` holds.
#[derive(Default)]
struct Held {
    admitted: bool,
    given: Option<Given>,
}

/// The rows given a delta table, of the table named, of these columns.
pub type Given = (TransitionRows, QualifiedName, Vec<ColumnDesc>);

impl fmt::Debug for DeltaRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeltaRows")
    }
}

impl DeltaRows {
    /// Marks it as standing where its statement is run.
    pub fn admit(&self) {
        if let Ok(mut held) = self.0.lock() {
            held.admitted = true;
        }
    }

    /// Whether it stands where its statement is run.
    pub fn admitted(&self) -> bool {
        self.0.lock().is_ok_and(|held| held.admitted)
    }

    /// Its rows, if its statement ran.
    pub fn given(&self) -> Option<Given> {
        self.0.lock().ok().and_then(|held| held.given.clone())
    }

    /// Gives it `given`, its statement's rows.
    pub fn give(&self, given: Given) {
        if let Ok(mut held) = self.0.lock() {
            held.given = Some(given);
        }
    }
}

impl super::QueryExpression {
    /// \[DESIGN\] The delta table that is the one table reference of its
    /// FROM clause, if it is a query specification, ordered or not, of
    /// one (RULES.md D174).
    pub fn delta(&self) -> Option<&DeltaTable> {
        let mut query = self;
        while let super::QueryExpression::Ordered(o) = query {
            query = &o.body;
        }
        let super::QueryExpression::Spec(spec) = query else { return None };
        if let [super::TableReference::Delta(d)] = spec.table_expression.from.as_slice() { Some(d) } else { None }
    }

    /// [ISO/IEC 9075-2:2023 §7.17 SR2, SR3d iv, GR1a; §7.19 SR1] The delta
    /// tables whose statements it runs, in the order they run: that of
    /// each `<with list element>` of its WITH clause, in order, then its
    /// own (RULES.md D175). Each is the one table reference of the FROM
    /// clause of a query specification, ordered or not, as `delta` finds.
    /// Anywhere else, in a subquery, in a derived table, beside another
    /// table reference, or in an operand of a set operator, a delta table
    /// is none of these, and is refused as it is bound.
    pub fn deltas(&self) -> Vec<&DeltaTable> {
        let mut query = self;
        while let super::QueryExpression::Ordered(o) = query {
            query = &o.body;
        }
        let super::QueryExpression::With(w) = query else { return self.delta().into_iter().collect() };
        let mut found: Vec<&DeltaTable> = w.elements.iter().filter_map(|e| e.query.delta()).collect();
        found.extend(w.body.deltas());
        found
    }

    /// Marks each of `deltas` as standing where its statement is run.
    pub fn admit_deltas(&self) {
        for d in self.deltas() {
            d.rows.admit();
        }
    }
}
