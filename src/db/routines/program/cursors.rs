//! [ISO/IEC 9075-4:2006 draft §12.1 to §12.4, §14.1 GR4, GR5] The local
//! cursors of a program the session runs: session cursors of names no
//! `<cursor name>` is (`binder/psm.rs`, `cursor_key`), declared as their compound
//! statement or FOR statement begins and destroyed as it ends.

use super::super::super::dynamic::Name;
use super::super::super::{DeclaredCursor, Session, StatementContext, rule};
use crate::ast::FetchOrientation;
use crate::error::Result;
use crate::logical::Cursor;
use crate::params::Params;
use crate::storage::Row;

impl Session {
    /// [§12.1] Declares the local cursor `cursor` as `key`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a cursor of the name is declared, as no other
    /// execution's can be.
    pub(in crate::db) fn declare_local(&self, key: &str, cursor: &Cursor) -> Result<()> {
        let mut inner = self.inner.borrow_mut();
        let name = Name::plain(key);
        if inner.cursors.contains_key(&name) {
            return Err(rule(format!("[ISO/IEC 9075-4:2006 draft §14.1 SR15] cursor {} is already declared", cursor.name)));
        }
        let declared = DeclaredCursor {
            specification: cursor.text.clone(),
            insensitive: cursor.insensitive,
            scroll: cursor.scroll,
            updatable: None,
            open: None,
            statement: None,
        };
        inner.cursors.insert(name, declared);
        Ok(())
    }

    /// [§12.2] Opens the local cursor `key`, its specification's values
    /// those of `frame`.
    ///
    /// # Errors
    ///
    /// As OPEN's.
    pub(in crate::db) fn open_local(&self, cx: StatementContext<'_>, key: &str, cursor: &Cursor, frame: &Params) -> Result<()> {
        let flags = (cursor.insensitive, cursor.scroll);
        self.open_text(cx, &Name::plain(key), (&cursor.text, flags, None), frame).map(|_| ())
    }

    /// [§12.3] The row the local cursor `key` fetches in `orientation`;
    /// none past its rows.
    ///
    /// # Errors
    ///
    /// As FETCH's.
    pub(in crate::db) fn fetch_local(
        &self,
        cx: StatementContext<'_>,
        key: &str,
        orientation: &FetchOrientation,
        frame: &Params,
    ) -> Result<Option<Row>> {
        let ((), fetched) = self.fetch_row(cx, (&Name::plain(key), orientation), frame, |_, _| Ok(()))?;
        Ok(fetched.map(|(_, row)| row))
    }

    /// [§12.4] Closes the local cursor `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 24000 if it is not open.
    pub(in crate::db) fn close_local(&self, key: &str) -> Result<()> {
        self.close(&Name::plain(key)).map(|_| ())
    }

    /// [§14.1 GR4, GR5] The local cursor `key` closed, if it is open, and
    /// destroyed.
    pub(in crate::db) fn destroy_local(&self, key: &str) {
        self.inner.borrow_mut().cursors.remove(&Name::plain(key));
    }
}
