//! \[EXTENSION\] CREATE INDEX, DROP INDEX and ALTER INDEX in SQL (RULES.md
//! D34, D41): the statements of `Database::create_index`, `drop_index` and
//! the index options. An index is built while transactions go on writing
//! its table (`db/indexes.rs`), so these run outside a transaction, as
//! PostgreSQL's CREATE INDEX CONCURRENTLY does, each made at once; and only
//! by the owner of the table's schema, as ALTER TABLE [ISO/IEC 9075:1992
//! §11.10 AR1]. An index takes no UNIQUE: a UNIQUE constraint makes its own
//! [X3.135-1986 §6.4]. Split from db.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::alter::value_of;
use super::indexes::IndexSpec;
use super::{Completion, Session, State};
use crate::ast::{IndexStatement, TableOptionValue};
use crate::binder::qualify;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::error::{At, Code, Result, SqlError, fail};
use crate::index::BTREE_METHOD;
use crate::storage::chosen;
use crate::value::Value;

/// A Syntax Rule of this extension broken (SQLCODE -103).
fn refused(message: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {message}"))
}

impl Session {
    /// \[EXTENSION\] Runs the index statement `stmt` on `st`.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 in a transaction; -104 if the session's identifier does
    /// not own the table's schema; -103 for a table, column, method, index
    /// or option not there, or a value not of its option's kind; those of
    /// `Database::create_index`, `drop_index`.
    pub(super) fn index_statement(&self, st: &Arc<State>, stmt: &IndexStatement) -> Result<Completion> {
        if self.inner.borrow().tx.is_some() {
            return fail(
                Code::TransactionState,
                "[EXTENSION] an index statement runs outside a transaction: commit or roll back first".to_owned(),
            );
        }
        match stmt {
            IndexStatement::Create { name, table, columns, storage } => {
                let qn = qualify(table, &self.default_schema());
                self.owns(st, &qn)?;
                let desc = st.base_table(&qn)?;
                for (column, at) in columns {
                    if desc.column(column).is_none() {
                        return Err(refused(&format!("{qn} has no column {column}"))).or_at(*at);
                    }
                }
                let method = match &storage.engine {
                    Some((given, at)) => method_named(st, given).or_at(*at)?,
                    None => BTREE_METHOD.to_owned(),
                };
                if st.catalog.index(name).is_some() {
                    return Err(refused(&format!("an index named {name} already exists")));
                }
                let options = options_of(st, &method, &storage.options)?;
                let columns = columns.iter().map(|(c, _)| c.clone()).collect();
                let spec = IndexSpec { name: name.clone(), table: qn, columns, method, options };
                self.db.noted(self.db.make_index(&spec))?;
            }
            IndexStatement::Drop { name: (name, at) } => {
                let ix = self.index_owned(st, name).or_at(*at)?;
                if ix.unique {
                    return Err(refused(&format!("index {name} is a UNIQUE constraint's: drop the constraint"))).or_at(*at);
                }
                self.db.drop_index(name)?;
            }
            IndexStatement::Alter { name: (name, at), options } => {
                self.index_owned(st, name).or_at(*at)?;
                let given: Vec<(String, Option<Value>)> =
                    options.iter().map(|o| (o.name.clone(), o.value.as_ref().map(value_of))).collect();
                self.db.alter_index(name, &given)?;
            }
        }
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §11.10 AR1] Its identifier owns the schema of
    /// table `qn`.
    fn owns(&self, st: &State, qn: &QualifiedName) -> Result<()> {
        if self.owns_schema(st, &qn.auth) {
            return Ok(());
        }
        fail(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.10 AR1] {qn} is in a schema {} does not own", self.auth()))
    }

    /// The index `name`, of a table whose schema its identifier owns.
    fn index_owned(&self, st: &State, name: &str) -> Result<IndexDesc> {
        let ix = st.catalog.index(name).cloned().ok_or_else(|| refused(&format!("there is no index {name}")))?;
        self.owns(st, &ix.table)?;
        Ok(ix)
    }
}

/// The registered index method named `name`, in any case.
fn method_named(st: &State, name: &str) -> Result<String> {
    let found = st.methods.keys().find(|k| k.eq_ignore_ascii_case(name));
    found.cloned().ok_or_else(|| refused(&format!("there is no index method {name}")))
}

/// The options `given` of an index of `method`, taken of their kinds.
///
/// # Errors
///
/// SQLCODE -103 for an option the method does not declare, one given
/// twice, or a value not of its kind, at the option.
fn options_of(st: &State, method: &str, given: &[TableOptionValue]) -> Result<Vec<(String, Value)>> {
    let declared = st.index_options_of(method);
    let mut options = Vec::new();
    for (n, option) in given.iter().enumerate() {
        if given.iter().take(n).any(|o| o.name.eq_ignore_ascii_case(&option.name)) {
            return Err(refused(&format!("the index option {} is given twice", option.name))).or_at(option.at);
        }
        let value = option.value.as_ref().map(value_of);
        options = chosen(method, (&declared, &options), (&option.name, value.as_ref())).or_at(option.at)?;
    }
    Ok(options)
}
