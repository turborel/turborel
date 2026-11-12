//! [ISO/IEC 9075-2:2023 §6.14] NEXT VALUE FOR: where it may be, the
//! sequence generator it names, and USAGE on it.
//!
//! It is directly in the select list of a `<query specification>` that is
//! the whole query of a cursor, a SELECT INTO or an INSERT, in the rows an
//! INSERT's VALUES gives, or in an UPDATE's SET source \[SR1\]; never in a
//! CASE, a search condition, ORDER BY, a set function, a grouped query or
//! one of DISTINCT \[SR2\]. So no view, constraint or default holds one, and
//! nothing a schema holds depends on a sequence generator.

use std::sync::Arc;

use super::{Binder, qualify};
use crate::ast::TableName;
use crate::catalog::owner_of;
use crate::error::{Code, Result, SqlError};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.14] NEXT VALUE FOR `name`: of the type of the
    /// sequence generator it names \[SR3\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is where it may not be \[SR1, SR2\], or names no
    /// sequence generator; -104 if the current privileges hold no USAGE on
    /// it \[AR1b\].
    pub(super) fn next_value(&self, name: &TableName) -> Result<(Expr, DataType)> {
        if !self.drawing.get() {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 9075-2:2023 §6.14 SR1, SR2] NEXT VALUE FOR is only directly in the select list of the whole query of a cursor, a SELECT INTO or an INSERT, not grouped nor DISTINCT, in an INSERT's VALUES or in an UPDATE's SET, and not in a CASE, a search condition, ORDER BY or a set function",
            ));
        }
        let qn = qualify(name, self.schema);
        let seq = self.db.sequence_named(&qn, "§6.14 SR3")?;
        if !seq.usage_of(self.auth, owner_of(&*self.db.catalog, &qn.auth)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §6.14 AR1b] {} holds no USAGE on sequence generator {qn}", self.auth);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        let t = seq.data_type;
        Ok((Expr::NextValue(Arc::new(seq)), t))
    }

    /// `bind`, run with what it binds drawing values of sequence generators
    /// if `drawing`; as before after it.
    pub(super) fn drawing<T>(&self, drawing: bool, bind: impl FnOnce() -> T) -> T {
        let before = self.drawing.replace(drawing);
        let bound = bind();
        self.drawing.set(before);
        bound
    }
}
