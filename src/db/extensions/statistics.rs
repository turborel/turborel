//! \[EXTENSION\] The statistics a plug-in asks of the database (RULES.md
//! D279): a table's and each of its columns', one column's, or an
//! index's, as the newest state of the database keeps them, the same the
//! planner reads. A table not analyzed, or an engine that keeps none,
//! answers none known. Privileges are the pool's to check
//! (`Pool::statistics`): a call that runs SQL is answered by its user's,
//! another by the server's, as extensions run (D173).

use super::super::{Database, State, qualified_name};
use crate::error::Result;
use crate::plugin::errors::violation;
use crate::plugin::host::Answered;
use crate::plugin::payload::statistics::{Asked, IndexAnswer, TableAnswer, answer_of, or_unknown, put_index, put_table};

impl Database {
    /// Registers what answers the statistics plug-ins ask: the newest
    /// state's, while the database is open.
    pub(in crate::db) fn serve_statistics(&self) {
        let weak = self.downgrade();
        if let Ok(st) = self.snapshot() {
            st.extension_host.set_statistician(std::sync::Arc::new(move |asked: &Asked| {
                let db = weak.upgrade().ok_or_else(|| violation("statistics are asked of a database closed"))?;
                told(&*db.snapshot()?, asked)
            }));
        }
    }
}

/// What `st` keeps of what `asked` names.
///
/// # Errors
///
/// SQLSTATE 38T03 of a table, column or index there is not, as the
/// program asked wrongly; as `put_table`.
fn told(st: &State, asked: &Asked) -> Result<Answered> {
    match asked {
        Asked::Table(t) => {
            let (name, answer) = table(st, t)?;
            let mut payload = Vec::new();
            put_table(&mut payload, answer.as_ref())?;
            Ok(Answered { table: (name.auth, name.ident), column: None, payload })
        }
        Asked::Column(t, c) => {
            let (name, answer) = table(st, t)?;
            let desc = st.catalog.table(&name).ok_or_else(|| violation(&format!("there is no table {t}")))?;
            let at = desc.columns.iter().position(|col| col.name == *c).ok_or_else(|| violation(&format!("{t} has no column {c}")))?;
            let one = answer.map(|a| TableAnswer { columns: a.columns.get(at).cloned().into_iter().collect(), ..a });
            let mut payload = Vec::new();
            put_table(&mut payload, one.as_ref())?;
            Ok(Answered { table: (name.auth, name.ident), column: Some(c.clone()), payload })
        }
        Asked::Index(i) => {
            let ix = st.catalog.index(i).ok_or_else(|| violation(&format!("there is no index {i}")))?;
            let index = st.index_of(&ix.name)?;
            let rows = st.row_count(&ix.table).map(crate::storage::cost_of);
            let answer = IndexAnswer {
                entries: or_unknown(rows),
                distinct: or_unknown(index.distinct_keys()),
                pages: or_unknown(index.probe_pages(&crate::index::Probe::All)),
                refresh: 0,
            };
            let mut payload = Vec::new();
            put_index(&mut payload, Some(&answer));
            Ok(Answered { table: (ix.table.auth.clone(), ix.table.ident.clone()), column: None, payload })
        }
    }
}

/// The table `t` names, written "SCHEMA.TABLE", and what `st` keeps of
/// it, if anything.
fn table(st: &State, t: &str) -> Result<(crate::catalog::QualifiedName, Option<TableAnswer>)> {
    let name = qualified_name(t).map_err(|_| violation(&format!("{t} is not written SCHEMA.TABLE")))?;
    let desc = st.catalog.table(&name).ok_or_else(|| violation(&format!("there is no table {t}")))?;
    let answer = st.statistics(&name).map(|s| answer_of(&*s, desc.columns.len()));
    Ok((name, answer))
}
