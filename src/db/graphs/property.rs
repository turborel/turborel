//! \[EXTENSION\] INTO PROPERTY (RULES.md D-GRAPH-ALGO): a graph
//! algorithm's result written as a property of the vertices of a table
//! [GS §18], in the statement's transaction. The column is added first,
//! by ALTER TABLE ADD COLUMN, if the table has none of its name, of DOUBLE
//! PRECISION, BIGINT or the type of the table's key; then the MERGE the
//! statement is read as sets it of each row whose key is a result row's
//! vertex. A row of no result row keeps its value: of a column just
//! added, null.

use std::sync::Arc;

use crate::ast::{IntoProperty, MergeStatement, SqlStatement};
use crate::binder::qualify;
use crate::db::{Completion, Session, StatementContext, rule};
use crate::error::{At, Result};
use crate::params::Params;
use crate::parser::Parser;

impl Session {
    /// The INTO PROPERTY `p` of the MERGE `stmt`, run in `cx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table names no key column and has no primary
    /// key of one column, or has no column of the key's name; as ALTER
    /// TABLE's and MERGE's otherwise.
    pub(in crate::db) fn property_written(
        &self,
        cx: StatementContext<'_>,
        stmt: &MergeStatement,
        p: &IntoProperty,
        params: &Params,
    ) -> Result<Completion> {
        let qn = qualify(&stmt.target, &self.default_schema());
        let desc = cx.st.base_table(&qn).or_at(stmt.target.at)?;
        let primary = || match desc.primary_key() {
            Some([one]) => desc.columns.get(*one).map(|c| c.name.clone()),
            Some(_) | None => None,
        };
        let key = p.key.clone().or_else(primary).ok_or_else(|| {
            rule(format!("[EXTENSION] INTO PROPERTY of {qn} names its key column, as {qn} has no primary key of one column"))
        })?;
        let Some(key_column) = desc.column(&key).and_then(|k| desc.columns.get(k)) else {
            return Err(rule(format!("[EXTENSION] INTO PROPERTY: {qn} has no column {key}")));
        };
        let st = if desc.column(&p.column).is_some() {
            Arc::clone(cx.st)
        } else {
            let typed = p.value_type.map_or_else(|| key_column.data_type.to_string(), str::to_owned);
            let alter = format!("ALTER TABLE {} ADD COLUMN {} {typed}", p.table, crate::catalog::identifier(&p.column));
            let SqlStatement::AlterTable { table, action, text } = Parser::new(&alter)?.sql_statement()? else {
                return Err(rule("[DESIGN] INTO PROPERTY adds its column by ALTER TABLE"));
            };
            self.alter_table(cx.st, cx.tx, (&table, &action, &text))?;
            self.current_state()?
        };
        let SqlStatement::Merge(merge) = Parser::new(&p.merge(&key))?.sql_statement()? else {
            return Err(rule("[DESIGN] INTO PROPERTY is a MERGE"));
        };
        self.merge_completed(StatementContext { st: &st, tx: cx.tx }, &MergeStatement { property: None, ..*merge }, params, None)
    }
}
