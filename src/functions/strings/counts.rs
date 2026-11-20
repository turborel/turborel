//! [ISO/IEC 9075:1992 §6.6; ISO/IEC 9075-2:2023 §6.31] The string
//! functions giving a number: POSITION, and the lengths CHAR_LENGTH,
//! OCTET_LENGTH and BIT_LENGTH. Of a large object, POSITION reads both
//! strings by position (`functions/lobs/search.rs`), and a length is its
//! counts, read from no chunk.

use super::shapes::shape;
use super::{COUNTED, beside, binary_string, character, counted, named, no_units, text};
use crate::error::{Code, Result, fail};
use crate::functions::lobs::ops::chars_before;
use crate::functions::lobs::search::{Octets, find};
use crate::functions::over_vectors::counted as counted_texts;
use crate::functions::{Arg, ScalarFunction};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::storage::Column;
use crate::value::{BYTE_BITS, CharUnits, DataType, Value};

/// [§6.6] `<position expression>`: POSITION(A IN B), called with A and B:
/// 1 if A is empty, else where A first starts in B, from 1, or 0 \[GR1\];
/// of binary strings, the same in octets [ISO/IEC 9075-2:2023 §6.31
/// GR3]; [Feature T062] where in the units written after USING,
/// characters if none.
pub(super) struct Position {
    pub(super) units: Option<CharUnits>,
}

/// The octets of `v`, a string, read by position; none if it is null.
fn octets_of(v: &Value) -> Result<Option<Octets<'_>>> {
    match v {
        Value::Null => Ok(None),
        Value::Char(s) => Ok(Some(Octets::Held(s.as_bytes()))),
        Value::Binary(b) => Ok(Some(Octets::Held(b.bytes()))),
        Value::Lob(l) => Ok(Some(Octets::of_lob(l))),
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
        | Value::Xml(_) => fail(Code::Api, "[DESIGN] POSITION was given a value not a string"),
    }
}

impl Position {
    /// [ISO/IEC 9075-2:2023 §6.31 GR2, GR3] POSITION of `a` in `b`,
    /// either a large object, neither held whole.
    fn large(&self, a: &Value, b: &Value) -> Result<Value> {
        let (Some(mut needle), Some(mut hay)) = (octets_of(a)?, octets_of(b)?) else { return Ok(Value::Null) };
        // [GR2a, GR3a]
        if needle.len() == 0 {
            return Ok(counted(1));
        }
        // [GR2b, GR3b] a match of UTF-8 octets begins at a character, so
        // the first in octets is the first in characters.
        let Some(at) = find(&mut needle, &mut hay)? else { return Ok(counted(0)) };
        let before = match (b, self.units) {
            (Value::Lob(l), None | Some(CharUnits::Characters)) => chars_before(l, at)?,
            (Value::Char(s), None | Some(CharUnits::Characters)) => {
                let n = s.get(..usize::try_from(at).unwrap_or(usize::MAX)).map_or(0, |x| x.chars().count());
                u64::try_from(n).unwrap_or(u64::MAX)
            }
            (_, Some(CharUnits::Octets)) | (Value::Binary(_), _) => at,
            (
                Value::Null
                | Value::Exact(_)
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
                | Value::Xml(_),
                _,
            ) => return Ok(Value::Null),
        };
        Ok(counted(before.saturating_add(1)))
    }
}

impl ScalarFunction for Position {
    fn name(&self) -> &'static str {
        named(kw::POSITION, self.units)
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] POSITION takes two strings") };
        // [ISO/IEC 9075-2:2023 §6.31 SR2] binary strings both, or
        // character strings, a large object among them or not.
        if let (Some(x), Some(y)) = (shape(a), shape(b)) {
            if x.kind != y.kind {
                return fail(
                    Code::SyntaxRule,
                    format!("[ISO/IEC 9075-2:2023 §6.31 SR2] POSITION takes two strings of one family, not {a} and {b}"),
                );
            }
            if a.is_binary() {
                no_units(kw::POSITION, self.units)?;
            }
            return Ok(COUNTED);
        }
        character(self.name(), a)?;
        character(self.name(), b)?;
        Ok(COUNTED)
    }
    /// [§17.6 GR5c] CHARACTER VARYING, or BINARY VARYING beside a binary
    /// string, unless the other string is one too \[GR4l\].
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        beside(position, args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] POSITION takes two strings") };
        if let (Value::Binary(a), Value::Binary(b)) = (a, b) {
            return Ok(counted(u64::from(b.position(a))));
        }
        if matches!(a, Value::Lob(_)) || matches!(b, Value::Lob(_)) {
            return self.large(a, b);
        }
        let (Some(a), Some(b)) = (text(a)?, text(b)?) else { return Ok(Value::Null) };
        if a.is_empty() {
            return Ok(counted(1));
        }
        // [ISO/IEC 9075-2:2023 §6.31 GR2b] in octets, where in B's UTF-8.
        let before = |at: usize| if self.units == Some(CharUnits::Octets) { at } else { b.get(..at).map_or(0, |x| x.chars().count()) };
        Ok(counted(b.find(a).map_or(0, |at| u64::try_from(before(at)).unwrap_or(u64::MAX).saturating_add(1))))
    }
}

/// What a length counts.
#[derive(Clone, Copy)]
pub(super) enum Unit {
    /// Characters [§6.6 GR4].
    Characters,
    /// Octets of the string as UTF-8, its implementation-defined form of use
    /// \[GR5\]; of a binary string, its octets [ISO/IEC 9075-2:2023 §6.31
    /// GR6].
    Octets,
    /// \[EXTENSION\] Bits of those octets, as SQL-92 counts them \[GR6\];
    /// SQL:2023 has no BIT_LENGTH.
    Bits,
}

/// [§6.6] `<length expression>`: CHAR_LENGTH (or CHARACTER_LENGTH),
/// OCTET_LENGTH or BIT_LENGTH; [Feature T062] CHAR_LENGTH USING
/// CHARACTERS or OCTETS, which count as CHAR_LENGTH and OCTET_LENGTH
/// [ISO/IEC 9075-2:2023 §6.31 GR5b].
pub(super) struct Length {
    pub(super) name: &'static str,
    pub(super) unit: Unit,
    /// Whether it takes a binary string too.
    pub(super) binary: bool,
}

impl ScalarFunction for Length {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] a length takes one string") };
        // [ISO/IEC 9075-2:2023 §6.31] CHAR_LENGTH of a character string
        // alone; OCTET_LENGTH of any string.
        // [ISO/IEC 9075-2:2023 §6.31 SR1, SR2] of a large object too, which
        // its counts give.
        let large = if self.binary { s.is_lob() } else { s.is_lob() && s.is_character() };
        let binary = self.binary && binary_string(s).is_some();
        if !(binary || large) {
            character(self.name, s)?;
        }
        Ok(COUNTED)
    }
    /// [§17.6 GR5b] CHARACTER VARYING.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Varying)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] a length takes one string") };
        let octets = match s {
            Value::Binary(b) => u64::try_from(b.bytes().len()).unwrap_or(u64::MAX),
            // Its counts, read from no chunk.
            Value::Lob(l) => {
                if matches!(self.unit, Unit::Characters) {
                    l.chars()
                } else {
                    l.octets()
                }
            }
            Value::Char(c) => {
                let n = if matches!(self.unit, Unit::Characters) { c.chars().count() } else { c.len() };
                u64::try_from(n).unwrap_or(u64::MAX)
            }
            Value::Null => return Ok(Value::Null),
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
            | Value::Xml(_) => {
                return text(s).map(|_| Value::Null);
            }
        };
        Ok(counted(if matches!(self.unit, Unit::Bits) { octets.saturating_mul(u64::from(BYTE_BITS)) } else { octets }))
    }
    /// \[DESIGN\] (RULES.md D363) Of a vector of character strings, each
    /// counted in characters or octets, as `call` counts it.
    fn eval_batch(&self, args: &[Arg<'_>], _rows: usize) -> Option<Result<Column>> {
        let [Arg::Column(Column::Texts(texts))] = args else { return None };
        let made = match self.unit {
            Unit::Characters => counted_texts(texts, |s| s.chars().count()),
            Unit::Octets => counted_texts(texts, str::len),
            Unit::Bits => return None,
        };
        Some(Ok(Column::Ints(made)))
    }
}
