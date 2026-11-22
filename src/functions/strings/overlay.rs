//! [ISO/IEC 9075-2:2023 §6.33] `<character overlay function>` and `<binary
//! overlay function>` (Feature T312): OVERLAY(CV PLACING RS FROM SP \[FOR
//! SL\] \[USING units\]), called with CV, RS, SP and, if written, SL. It
//! is SUBSTRING and concatenation \[SR17c, SR23c\], so it is made of them,
//! of their types and values, and of a large object it slices and joins,
//! copying nothing (RULES.md D79).

use super::shapes::shape;
use super::{Concatenation, Substring, beside, counted, integer, named, no_units, whole};
use crate::error::{Code, Result, fail};
use crate::functions::ScalarFunction;
use crate::params::ParameterType;
use crate::value::{BIGINT_PRECISION, CharUnits, DataType, Decimal, Spelled, Value};

/// [ISO/IEC 9075-2:2023 §6.33] OVERLAY's name, no key word of SQL-92.
pub const OVERLAY: &str = "OVERLAY";

/// [ISO/IEC 9075-2:2023 §6.33] The word before an overlay's replacement.
pub const PLACING: &str = "PLACING";

/// OVERLAY, its positions in the units written after USING, characters if
/// none.
pub(super) struct Overlay {
    pub(super) units: Option<CharUnits>,
}

/// `n` as an integer value; null if none.
fn integer_value(n: Option<i128>) -> Value {
    n.map_or(Value::Null, |n| Value::Exact(Decimal::new(n, 0)))
}

impl Overlay {
    /// \[SR17b, SR23b\] SL: as written, else the length of RS, in the units
    /// written after USING (\[DESIGN\] SR17b says CHAR_LENGTH(RS), which
    /// is in characters, but SUBSTRING then counts octets); null if RS is.
    fn replaced_length(&self, rs: &Value, written: Option<&Value>) -> Result<Value> {
        if let Some(sl) = written {
            return Ok(sl.clone());
        }
        let octets = self.units == Some(CharUnits::Octets);
        Ok(match rs {
            Value::Null => Value::Null,
            Value::Char(s) => counted(u64::try_from(if octets { s.len() } else { s.chars().count() }).unwrap_or(u64::MAX)),
            Value::Binary(b) => counted(u64::from(b.len())),
            Value::Lob(l) => counted(if octets { l.octets() } else { l.chars() }),
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => return fail(Code::Api, "[DESIGN] OVERLAY was given a replacement not a string"),
        })
    }
}

impl ScalarFunction for Overlay {
    fn name(&self) -> &'static str {
        named(OVERLAY, self.units)
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let ([cv, rs, sp] | [cv, rs, sp, _]) = args else {
            return fail(Code::Api, "[DESIGN] OVERLAY takes a string, a string and positions");
        };
        for t in args.get(2..).unwrap_or_default() {
            integer(OVERLAY, t)?;
        }
        // [SR17a, SR23a] two character strings or two binary strings, as
        // the formats say.
        if let (Some(x), Some(y)) = (shape(cv), shape(rs))
            && x.kind != y.kind
        {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §6.33 SR17, SR23] OVERLAY places a string of {cv}'s family, not {rs}"),
            );
        }
        if cv.is_binary() {
            no_units(OVERLAY, self.units)?;
        }
        // [SR17c, SR23c] SUBSTRING (CV FROM 1 FOR SP - 1) || RS ||
        // SUBSTRING (CV FROM SP + SL).
        let cut = Substring { units: self.units }.return_type(&[cv.clone(), sp.clone()])?;
        let left = Concatenation.return_type(&[cut.clone(), rs.clone()])?;
        Concatenation.return_type(&[left, cut])
    }
    /// [ISO/IEC 9075:1992 §17.6 GR5c] The start and the length
    /// NUMERIC(P, 0), P the greatest precision, as SUBSTRING's; a string
    /// beside the other as `||`'s operand.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        if position > 1 {
            return Some(ParameterType::Of(DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Numeric }));
        }
        beside(position, args)
    }
    /// \[GR3, GR17; §6.32 GR2, GR3\] As SUBSTRING and `||` give it: null if
    /// any operand is; SQLSTATE 22011 if SP is less than 1, as the length
    /// of the first SUBSTRING is SP - 1.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let ([cv, rs, sp] | [cv, rs, sp, _]) = args else {
            return fail(Code::Api, "[DESIGN] OVERLAY takes a string, a string and positions");
        };
        let start = whole(Some(sp))?;
        let sl = self.replaced_length(rs, args.get(3))?;
        let sl = whole(Some(&sl))?;
        let cut = Substring { units: self.units };
        let head = cut.call(&[cv.clone(), counted(1), integer_value(start.map(|s| s.saturating_sub(1)))])?;
        let tail = cut.call(&[cv.clone(), integer_value(start.zip(sl).map(|(s, l)| s.saturating_add(l)))])?;
        let left = Concatenation.call(&[head, rs.clone()])?;
        Concatenation.call(&[left, tail])
    }
}
