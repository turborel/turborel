//! [ISO/IEC 9075-2:2023 §9.23 SR5, SR6] NTILE, LEAD, LAG, FIRST_VALUE,
//! LAST_VALUE and NTH_VALUE, bound: a count of tiles or rows an exact
//! number of scale 0 over no columns, the extent or argument in the select
//! list's scope, window functions among it, and LEAD and LAG's default of
//! the extent's type.

use super::super::composite::integer;
use super::super::statements::assignable;
use super::super::{Binder, Scope, rule};
use crate::ast::{OffsetFunction, ValueExpression, ValueFunction, Which};
use crate::error::Result;
use crate::logical::{Expr, Offset, Pick, Valued, WindowKind};
use crate::params::ParameterType;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §9.23 SR5j ii] The offset of LEAD and LAG of none
/// written.
const ONE_ROW: u32 = 1;

/// [ISO/IEC 9075-2:2023 §9.23 SR5j iii] \[EXTENSION\] Whether a default of
/// type `t` is compatible with an extent of type `extent`: each assignable
/// to the other. The standard's "compatible" asks the same data type name
/// too, which would refuse `LAG(I, 1, 0)` of an INTEGER I, as a literal is
/// NUMERIC; the default is cast to the extent's type instead.
fn compatible(extent: &DataType, t: &DataType) -> bool {
    assignable(extent, t) && assignable(t, extent)
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §9.23 SR5i] NTILE of `tiles`, of COUNT's type
    /// (IV084).
    pub(super) fn ntile(&self, tiles: &ValueExpression) -> Result<(WindowKind, DataType)> {
        let tiles = self.number_of_rows(tiles, "SR5i i")?;
        Ok((WindowKind::Ntile(tiles), self.count_type()?))
    }

    /// [ISO/IEC 9075-2:2023 §9.23 SR5j] LEAD or LAG `o`, bound in `scope`,
    /// of its extent's type [SR5j v].
    pub(super) fn offset_function(&self, o: &OffsetFunction, scope: Scope<'_>) -> Result<(WindowKind, DataType)> {
        let (extent, t) = self.value_expression(&o.extent, scope)?;
        let default = match &o.default {
            // [SR5j iii 2] CAST (NULL AS DT).
            None => Expr::Const(Value::Null),
            Some(d) => {
                // [EXTENSION] a `?` takes the extent's type, where
                // [ISO/IEC 9075-2:2023 §20.7 GR5a] gives it none.
                let bound = self.typed(d, ParameterType::Of(t.clone()), scope)?;
                if !compatible(&t, &bound.1) {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §9.23 SR5j iii] the default of LEAD or LAG is of a type compatible with its extent's, not {} of {t}",
                        bound.1
                    )));
                }
                self.cast_bound(bound, t.clone())?.0
            }
        };
        let offset = o.offset.unwrap_or(ONE_ROW);
        Ok((WindowKind::Offset(Offset { lead: o.lead, extent, offset, default, ignore_nulls: o.ignore_nulls }), t))
    }

    /// [ISO/IEC 9075-2:2023 §9.23 SR6] FIRST_VALUE, LAST_VALUE or NTH_VALUE
    /// `v`, bound in `scope`, of its argument's type [SR6c].
    pub(super) fn value_function(&self, v: &ValueFunction, scope: Scope<'_>) -> Result<(WindowKind, DataType)> {
        let (arg, t) = self.value_expression(&v.arg, scope)?;
        let pick = match &v.which {
            Which::First => Pick::First,
            Which::Last => Pick::Last,
            Which::Nth { row, from_last } => Pick::Nth { row: self.number_of_rows(row, "SR6a ii")?, from_last: *from_last },
        };
        Ok((WindowKind::Value(Valued { pick, arg, ignore_nulls: v.ignore_nulls }), t))
    }

    /// The `<number of tiles>` or `<nth row>` `v`, over no columns, a `?`
    /// INTEGER [ISO/IEC 9075-2:2023 §20.7 GR5a xxxii]: exact numeric of
    /// scale 0, else the rule `rule_of` of §9.23 is broken.
    fn number_of_rows(&self, v: &ValueExpression, rule_of: &str) -> Result<Expr> {
        let none = Scope::Rows("[ISO/IEC 9075-2:2023 §6.10] a <number of tiles> or <nth row> is a <simple value specification>");
        let (e, t) = self.typed(v, ParameterType::Of(integer()), none)?;
        if !matches!(t, DataType::Exact { scale: 0, .. }) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.23 {rule_of}] the number of tiles or rows is exact numeric of scale 0, not {t}"
            )));
        }
        Ok(e)
    }
}
