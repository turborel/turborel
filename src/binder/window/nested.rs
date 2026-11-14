//! [ISO/IEC 9075-2:2023 §6.11] Nested window functions, bound: only while
//! the aggregated argument of a window function's set function is
//! \[SR1\], each a placeholder column of its place among that call's, ROW_NUMBER
//! of a row marker of COUNT's type (IV085) \[SR2\], VALUE_OF of the type of
//! its value expression and default \[SR3\], an offset exact of scale 0
//! \[SR4\].

use super::super::case::set_type;
use super::super::{Binder, Scope, rule};
use crate::ast::{NestedWindow, RowMarker, ValueExpression};
use crate::error::Result;
use crate::logical::{Expr, Marker, Nested};
use crate::params::ParameterType;
use crate::value::{DataType, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The placeholder column of the first nested window function of
/// a call: far past the columns of any row, and short of the window
/// functions' (`PLACEHOLDER`), so the plan's placing of them alone moves
/// them.
pub(super) const NESTED: usize = usize::MAX >> 2_u32;

/// [ISO/IEC 9075-2:2023 §6.11 SR1] A nested window function elsewhere.
const OUTSIDE: &str = "[ISO/IEC 9075-2:2023 §6.11 SR1] a nested window function is only in the argument of a set function a window function \
                       contains";

impl Binder<'_> {
    /// `bind`, run as the aggregated argument of a window function's set
    /// function is bound, so nested window functions may be in it; and
    /// those bound in it, inner first.
    pub(super) fn nesting<T>(&self, bind: impl FnOnce() -> Result<T>) -> Result<(T, Vec<Nested>)> {
        let before = self.windowing.borrow_mut().as_mut().map(|w| w.nesting.replace(Vec::new()));
        let bound = bind();
        let nested = self.windowing.borrow_mut().as_mut().and_then(|w| std::mem::replace(&mut w.nesting, before.flatten()));
        Ok((bound?, nested.unwrap_or_default()))
    }

    /// `bind`, run as a set function of a group is bound, so no nested
    /// window function may be in it, though it is in a window function's
    /// argument [SR1, §9.20].
    pub(in crate::binder) fn unnested<T>(&self, bind: impl FnOnce() -> Result<T>) -> Result<T> {
        let before = self.windowing.borrow_mut().as_mut().and_then(|w| w.nesting.take());
        let bound = bind();
        if let (Some(w), Some(before)) = (self.windowing.borrow_mut().as_mut(), before) {
            w.nesting = Some(before);
        }
        bound
    }

    /// [ISO/IEC 9075-2:2023 §6.11] The nested window function `n` in
    /// `scope`: a placeholder column of its value, and its type.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not in the argument of a window function's
    /// set function \[SR1\], its offset is not exact of scale 0 \[SR4\], or its
    /// value expression and default have no type in common [SR3, §9.5].
    pub(in crate::binder) fn nested_window(&self, n: &NestedWindow, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if self.windowing.borrow().as_ref().is_none_or(|w| w.nesting.is_none()) {
            return Err(rule(OUTSIDE));
        }
        let delta = match &n.delta {
            None => None,
            Some((minus, offset)) => Some((*minus, self.row_marker_offset(offset)?)),
        };
        let (value, t) = match &n.value {
            None => (None, self.count_type()?),
            Some((value, default)) => {
                let (v, d, t) = self.value_of(value, default.as_deref(), scope)?;
                (Some((v, d)), t)
            }
        };
        let mut windowing = self.windowing.borrow_mut();
        let nesting = windowing.as_mut().and_then(|w| w.nesting.as_mut()).ok_or_else(|| rule(OUTSIDE))?;
        let k = nesting.len();
        nesting.push(Nested { marker: marker(n.marker), delta, value });
        Ok((Expr::Column(NESTED.saturating_add(k)), t))
    }

    /// [ISO/IEC 9075-2:2023 §6.11 SR4] A `<row marker offset>`, over no
    /// columns, a `?` NUMERIC(MP, 0) [§20.7 GR5a xxxiii].
    fn row_marker_offset(&self, offset: &ValueExpression) -> Result<Expr> {
        let none = Scope::Rows("[ISO/IEC 9075-2:2023 §6.11] a <row marker offset> is a <simple value specification>");
        let numeric = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };
        let (e, t) = self.typed(offset, ParameterType::Of(numeric), none)?;
        if !matches!(t, DataType::Exact { scale: 0, .. }) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.11 SR4] a <row marker offset> is exact numeric of scale 0, not {t}")));
        }
        Ok(e)
    }

    /// [ISO/IEC 9075-2:2023 §6.11 SR3] VALUE_OF's `value` and `default`,
    /// bound in `scope`, each cast to the type §9.5 gives the two, a `?`
    /// default of the value's type [§20.7 GR5a xxxiv]; and that type.
    fn value_of(&self, value: &ValueExpression, default: Option<&ValueExpression>, scope: Scope<'_>) -> Result<(Expr, Expr, DataType)> {
        let bound = self.value_expression(value, scope)?;
        let Some(given) = default else {
            let t = bound.1.clone();
            return Ok((bound.0, Expr::Const(Value::Null), t));
        };
        let defaulted = self.typed(given, ParameterType::Of(bound.1.clone()), scope)?;
        let t = set_type(&[bound.1.clone(), defaulted.1.clone()], "§6.11 SR3, §9.5")?;
        let (v, _) = self.cast_bound(bound, t.clone())?;
        let (d, _) = self.cast_bound(defaulted, t.clone())?;
        Ok((v, d, t))
    }
}

/// The logical marker of `m`.
fn marker(m: RowMarker) -> Marker {
    match m {
        RowMarker::BeginPartition => Marker::BeginPartition,
        RowMarker::BeginFrame => Marker::BeginFrame,
        RowMarker::CurrentRow => Marker::CurrentRow,
        RowMarker::FrameRow => Marker::FrameRow,
        RowMarker::EndFrame => Marker::EndFrame,
        RowMarker::EndPartition => Marker::EndPartition,
    }
}
