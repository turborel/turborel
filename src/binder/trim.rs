//! The trims bound [ISO/IEC 9075-2:2023 §6.33]: each compares the
//! characters of its source under the source's collation [SR14d, SR15c].

use std::sync::Arc;

use super::{Binder, Scope};
use crate::ast::ValueExpression;
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarRef, Trim, source_of};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// The trim `name` of `args`: the one registered, unless its source
    /// is of a collation other than the default, which it then compares
    /// characters under.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an argument is no character string [SR14, SR15],
    /// or the source's collation cannot be determined [§9.15 SR4].
    pub(super) fn trim(&self, name: &'static str, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let source = source_of(name).and_then(|i| args.get(i));
        let collation = match source {
            Some(s) => self.collation_of(&[self.derived(s)?])?,
            None => None,
        };
        let f = match collation {
            Some(collation) => ScalarRef(Arc::new(Trim { name, collation: Some(collation) })),
            None => self.db.functions.scalar(name).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {name} is not registered")))?,
        };
        let (exprs, types) = self.arguments(&*f.0, args, scope)?;
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }
}
