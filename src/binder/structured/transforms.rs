//! [ISO/IEC 9075-2:2023 §6.4 SR12, GR20, GR21] CURRENT_DEFAULT_TRANSFORM_GROUP
//! and CURRENT_TRANSFORM_GROUP_FOR_TYPE, bound: the SQL-session context's
//! transform group names as the statement is bound (RULES.md D81).

use super::super::Binder;
use crate::catalog::StructuredDesc;
use crate::logical::Expr;
use crate::value::{CharUnits, DataType, Value};

/// [ISO/IEC 9075-2:2023 §6.4 SR12] (IV133) The most characters of a
/// transform group's name, an `<identifier>`'s.
pub const TRANSFORM_GROUP_LENGTH: u32 = 128;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.4 GR20, GR21] The default transform group
    /// name, or of `udt` its group name (`TransformNames::for_udt`): the
    /// zero-length string if none is set.
    pub(in crate::binder) fn current_transform_group(&self, udt: Option<&StructuredDesc>) -> (Expr, DataType) {
        let names = self.own.transforms();
        let name = udt.map_or(names.default.as_str(), |t| names.for_udt(&t.name, &t.supertypes));
        let t = DataType::Varying { length: TRANSFORM_GROUP_LENGTH, units: CharUnits::Characters };
        (Expr::Const(Value::Char(name.into())), t)
    }
}
