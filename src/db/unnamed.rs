//! \[DESIGN\] Unnamed cursors: a `<cursor specification>` read in one
//! call, `Session::open_cursor`, as direct SQL's queries are, and a
//! prepared query's (`Session::open_prepared`), its rows read as they are
//! fetched (`db/stream.rs`). Split from db/cursor.rs (RULES.md R-SIZE-2).

use std::sync::{Arc, Mutex};

use super::stream::Stream;
use super::{Completion, Cursor, Session, State, rule};
use crate::ast::{CursorSpecification, Updatability};
use crate::binder::OutColumn;
use crate::error::Result;
use crate::params::Params;
use crate::txn::Txn;

impl Session {
    /// \[DESIGN\] Read a `<cursor specification>` [X3.135-1986 §8.3] in one call: an
    /// unnamed cursor's rows, as FETCH would return them but without assigning
    /// them to parameters. It runs in the session's transaction, and is
    /// closed when that transaction ends.
    ///
    /// # Errors
    ///
    /// Any error of the cursor specification or of reading its rows.
    pub fn open_cursor(&self, sql: &str) -> Result<Cursor> {
        // [ISO/IEC 9075:1992 §18.1 Table 22] a <direct select statement:
        // multiple rows>.
        self.open_with(("SELECT", ""), |call| self.open_in(call, sql, &Params::new()))
    }

    /// An unnamed cursor `open` opens in a call, as `open_cursor` opens
    /// one: the diagnostics area naming it as `command` says.
    pub(super) fn open_with(
        &self,
        command: (&'static str, &'static str),
        open: impl FnOnce(&super::Call<'_>) -> Result<Cursor>,
    ) -> Result<Cursor> {
        let call = self.enter()?;
        self.emptied(command);
        let opened = open(&call);
        // [ISO/IEC 9075:1992 §3.3.4.1] with a warning its rows raised as it
        // opened (`Session::warning`).
        let warned = Completion::Success.warned(self.raised());
        self.diagnosed(opened, |_| warned.sqlstate())
    }

    /// The cursor of `sql`, with the parameters `params`, in the call
    /// `call`.
    fn open_in(&self, call: &super::Call<'_>, sql: &str, params: &Params) -> Result<Cursor> {
        let timed = super::slow::Timed::of(call.state(), sql.to_owned(), &self.auth());
        let spec = Self::parse_cursor(call.state(), sql, params)?;
        Self::unnamed_only(&spec)?;
        self.unnamed(call, timed, |st, tx| self.open_rows(st, tx, &spec, (params, None)))
    }

    /// Refuses `spec`, the specification of an unnamed cursor, if it is
    /// FOR UPDATE: no positioned statement names an unnamed cursor.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is FOR UPDATE.
    pub(super) fn unnamed_only(spec: &CursorSpecification) -> Result<()> {
        if let Some(Updatability::Update(_)) = spec.updatability {
            return Err(rule(
                "[DESIGN] an unnamed cursor is read-only: no positioned statement names it, so it is not FOR UPDATE".to_owned(),
            ));
        }
        Ok(())
    }

    /// An unnamed cursor, opened in the call `call` and the session's
    /// transaction over the rows `rows` gives, its work timed by `timed`.
    pub(super) fn unnamed(
        &self,
        call: &super::Call<'_>,
        mut timed: Option<super::slow::Timed<String>>,
        rows: impl FnOnce(&Arc<State>, &Txn) -> Result<(Vec<OutColumn>, Stream)>,
    ) -> Result<Cursor> {
        let st = call.state();
        let tx = self.ensure_transaction(st);
        call.runs(&tx);
        tx.begin_statement(|| !self.cursors_open())?;
        let (columns, rows) = rows(st, &tx).map_err(|e| st.fail_statement(&tx, &e).err().unwrap_or(e))?;
        let rows = Arc::new(Mutex::new(Some(rows)));
        self.inner.borrow_mut().anonymous.push(Arc::downgrade(&rows));
        if let Some(opened) = timed.as_mut() {
            opened.paused();
        }
        Ok(Cursor { columns, rows, db: self.db.clone(), stop: Arc::clone(&self.stop), timed, zone: self.now.get().zone })
    }
}
