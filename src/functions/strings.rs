//! [ISO/IEC 9075:1992 §6.6, §6.7, §6.13] The string functions and
//! concatenation, as `ScalarFunction` plugins registered under their key
//! words (concatenation under `||`), so an implementation can be replaced,
//! as a set function can. The parser reads each with its own syntax and
//! calls it with its operands in order (`parser/primary/strings.rs`).
//!
//! The trims and pads are in `functions/trim.rs`, POSITION and the lengths
//! in `strings/counts.rs`, and OVERLAY in `strings/overlay.rs`.
//!
//! A result is of the type its Syntax Rules give: CHARACTER VARYING of
//! SUBSTRING [§6.7 SR2a], the string's type of UPPER and
//! LOWER [§6.7 SR3a], and of `||` fixed or varying as its operands are
//! [§6.13 SR3a]. Of binary strings [ISO/IEC 9075-2:2023 §4.4], `||`,
//! SUBSTRING, POSITION and OCTET_LENGTH take them too, as SQL:2023's rules
//! say. Of large objects [ISO/IEC 9075-2:2023 §6.32, §6.33] each gives a
//! large object of its family, never holding one whole (RULES.md D79):
//! `||` and SUBSTRING slice and join, and UPPER and LOWER write a chunk at
//! a time (`functions/lobs/ops.rs`).

mod counts;
mod overlay;
pub(super) mod shapes;

use std::sync::Arc;

use super::ScalarFunction;
use super::lobs::ops::{concatenation, fold, within_octets};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::value::lob::{Lob, LobKind, TempLobs};
use crate::value::{Binary, DataType, Decimal, MAX_BINARY_LENGTH, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, Value};
use crate::value::{CharUnits, Spelled};
use counts::{Length, Position, Unit};
pub use overlay::{OVERLAY, PLACING};
use shapes::{large, large_concatenation};

/// The key word under which concatenation is registered: an operator, so no
/// call by name reaches it.
pub const CONCATENATION: &str = "||";

/// The names CHAR_LENGTH, POSITION, SUBSTRING and OVERLAY are registered
/// under with USING CHARACTERS and USING OCTETS, which no call by name
/// reaches.
const USING: [(&str, CharUnits, &str); 8] = [
    (kw::CHAR_LENGTH, CharUnits::Characters, "CHAR_LENGTH USING CHARACTERS"),
    (kw::CHAR_LENGTH, CharUnits::Octets, "CHAR_LENGTH USING OCTETS"),
    (kw::POSITION, CharUnits::Characters, "POSITION USING CHARACTERS"),
    (kw::POSITION, CharUnits::Octets, "POSITION USING OCTETS"),
    (kw::SUBSTRING, CharUnits::Characters, "SUBSTRING USING CHARACTERS"),
    (kw::SUBSTRING, CharUnits::Octets, "SUBSTRING USING OCTETS"),
    (OVERLAY, CharUnits::Characters, "OVERLAY USING CHARACTERS"),
    (OVERLAY, CharUnits::Octets, "OVERLAY USING OCTETS"),
];

/// The name the function `name` is registered under with USING `units`;
/// none if it takes no USING.
pub fn using(name: &str, units: CharUnits) -> Option<&'static str> {
    USING.iter().find(|(n, u, _)| *n == name && *u == units).map(|(.., registered)| *registered)
}

/// The name of the function `name` takes with USING `units`.
fn named(name: &'static str, units: Option<CharUnits>) -> &'static str {
    units.and_then(|u| using(name, u)).unwrap_or(name)
}

/// Every string function, to register.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    let mut all: Vec<Arc<dyn ScalarFunction>> = vec![
        Arc::new(Concatenation),
        Arc::new(Fold { upper: true }),
        Arc::new(Fold { upper: false }),
        Arc::new(Length { name: kw::OCTET_LENGTH, unit: Unit::Octets, binary: true }),
        Arc::new(Length { name: kw::BIT_LENGTH, unit: Unit::Bits, binary: true }),
    ];
    for units in [None, Some(CharUnits::Characters), Some(CharUnits::Octets)] {
        let unit = if units == Some(CharUnits::Octets) { Unit::Octets } else { Unit::Characters };
        all.push(Arc::new(Substring { units }));
        all.push(Arc::new(Position { units }));
        all.push(Arc::new(Length { name: named(kw::CHAR_LENGTH, units), unit, binary: false }));
        all.push(Arc::new(overlay::Overlay { units }));
    }
    all
}

/// [ISO/IEC 9075-2:2023 §6.31, §6.33] Of the binary string function
/// `name`, no USING, which only a character string's takes.
fn no_units(name: &str, units: Option<CharUnits>) -> Result<()> {
    match units {
        Some(_) => fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.31, §6.33] {name} of binary strings takes no USING")),
        None => Ok(()),
    }
}

/// The type of a length or position [§6.6 SR2, SR5]: exact numeric with
/// implementation-defined precision, `MAX_EXACT_PRECISION`, and scale 0.
const COUNTED: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// The length of `t`, a character string type, its maximum length if it
/// is varying, or a Syntax Rule error.
pub(super) fn character(name: &str, t: &DataType) -> Result<u32> {
    match t {
        DataType::Character { length, .. } | DataType::Varying { length, .. } => Ok(*length),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => {
            fail(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §6.7, §6.13] {name} takes a character string, not {t}"))
        }
    }
}

/// What the length of `t`, a character string type, counts; characters
/// of any other.
fn units_of(t: &DataType) -> CharUnits {
    if let DataType::Character { units, .. } | DataType::Varying { units, .. } = t { *units } else { CharUnits::Characters }
}

/// The length of `t`, a binary string type, its maximum length if it is
/// varying, and whether it is; none if it is no binary string type.
fn binary_string(t: &DataType) -> Option<(u32, bool)> {
    match t {
        DataType::Binary { length } => Some((*length, false)),
        DataType::VarBinary { length } => Some((*length, true)),
        // A binary large object is of `functions/lobs.rs`.
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Lob { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => None,
    }
}

/// [§6.7 SR1] Whether `t` is exact numeric with scale 0.
fn integer(name: &str, t: &DataType) -> Result<()> {
    match t {
        DataType::Exact { scale: 0, .. } => Ok(()),
        DataType::Exact { .. }
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => fail(
            Code::SyntaxRule,
            format!("[ISO/IEC 9075:1992 §6.7 SR1] the start position and length of {name} are exact numbers of scale 0, not {t}"),
        ),
    }
}

/// The string of `v`, if it is one; none if it is null.
pub(super) fn text(v: &Value) -> Result<Option<&str>> {
    match v {
        Value::Null => Ok(None),
        Value::Char(s) => Ok(Some(s)),
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Extension(_) => fail(Code::Api, "[DESIGN] a string function was given a value not a character string"),
    }
}

/// The integer of `v`, if it is one; none if it is null.
pub(super) fn whole(v: Option<&Value>) -> Result<Option<i128>> {
    let not_integer = || fail(Code::Api, "[DESIGN] a string function was given a position that is not an integer");
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Exact(d)) => {
            if d.scale == 0 {
                Ok(Some(d.units))
            } else {
                not_integer()
            }
        }
        Some(
            Value::Char(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Extension(_),
        ) => not_integer(),
    }
}

/// `n`, a count, as a value.
fn counted(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The family of `v` if it is a large object.
fn lob_kind(v: &Value) -> Option<LobKind> {
    if let Value::Lob(l) = v { Some(l.kind()) } else { None }
}

/// [§6.13] `<concatenation>`: the characters of one string, then the
/// other's \[GR2\]; null if either is \[GR2a\]. Of the sum of their
/// lengths: varying if either is, else fixed \[SR3a\]; a large object if
/// either is [ISO/IEC 9075-2:2023 §6.32 SR3b i, SR6a i].
struct Concatenation;

impl ScalarFunction for Concatenation {
    fn name(&self) -> &'static str {
        CONCATENATION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] || takes two operands") };
        if let Some(t) = large_concatenation(a, b)? {
            return Ok(t);
        }
        if let (Some((x, xv)), Some((y, yv))) = (binary_string(a), binary_string(b)) {
            return binary_concatenation(x.saturating_add(y), xv || yv);
        }
        let length = character(CONCATENATION, a)?.saturating_add(character(CONCATENATION, b)?);
        // [ISO/IEC 9075-2:2023 §6.1 SR16] in octets if both are, else in
        // characters, which a length in octets bounds.
        let units = if units_of(a) == units_of(b) { units_of(a) } else { CharUnits::Characters };
        // [SR3a i] varying, at most the longest varying string.
        if matches!(a, DataType::Varying { .. }) || matches!(b, DataType::Varying { .. }) {
            return Ok(DataType::Varying { length: length.min(MAX_CHARACTER_LENGTH), units });
        }
        // [SR3a ii] fixed, no longer than the longest fixed string.
        if length > MAX_CHARACTER_LENGTH {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075:1992 §6.13 SR3a ii] || of CHARACTER({length}) is longer than {MAX_CHARACTER_LENGTH} characters"),
            );
        }
        Ok(DataType::Character { length, units })
    }
    /// [§17.6 GR5d] CHARACTER VARYING, or BINARY VARYING beside a binary
    /// string, unless the other operand is one too \[GR4c\].
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        beside(position, args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] || takes two operands") };
        // [ISO/IEC 9075-2:2023 §6.32 GR2b ii 3A, GR3b i] joined without a
        // copy, at most the longest large object.
        if let Some(kind) = lob_kind(a).or_else(|| lob_kind(b)) {
            return concatenation(a, b, kind);
        }
        if let (Value::Binary(a), Value::Binary(b)) = (a, b) {
            return within_most(a.concat(b));
        }
        if matches!(a, Value::Binary(_)) || matches!(b, Value::Binary(_)) {
            return Ok(Value::Null);
        }
        Ok(match (text(a)?, text(b)?) {
            (Some(a), Some(b)) => Value::Char(format!("{a}{b}")),
            (None, _) | (_, None) => Value::Null,
        })
    }
}

/// [ISO/IEC 9075-2:2023 §6.32 SR6a] The type of `<binary
/// concatenation>`: of `length` octets, varying if an operand is, at most
/// the longest varying binary string \[SR6a ii\]; else fixed, no longer than
/// the longest fixed one \[SR6a iii\].
fn binary_concatenation(length: u32, varying: bool) -> Result<DataType> {
    if varying {
        return Ok(DataType::VarBinary { length: length.min(MAX_BINARY_LENGTH) });
    }
    if length > MAX_BINARY_LENGTH {
        return fail(
            Code::SyntaxRule,
            format!("[ISO/IEC 9075-2:2023 §6.32 SR6a iii] || of BINARY({length}) is longer than {MAX_BINARY_LENGTH} octets"),
        );
    }
    Ok(DataType::Binary { length })
}

/// [ISO/IEC 9075-2:2023 §6.32 GR3b ii] `s`, joined, if no longer than the
/// longest varying binary string; else its first octets, if those after
/// are all X'00'. Joined fixed strings are never longer \[SR6a iii\].
///
/// # Errors
///
/// SQLSTATE 22001 if an octet after them is not X'00'.
fn within_most(s: Binary) -> Result<Value> {
    let past = s.bytes().get(usize::try_from(MAX_BINARY_LENGTH).unwrap_or(usize::MAX)..).unwrap_or_default();
    if past.iter().any(|&o| o != 0) {
        return fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[ISO/IEC 9075-2:2023 §6.32 GR3b ii 3] || of {} octets is longer than {MAX_BINARY_LENGTH}", s.len()),
        );
    }
    Ok(Value::Binary(if past.is_empty() { s } else { s.substring(1, MAX_BINARY_LENGTH) }))
}

/// [§17.6 GR5] The type of a `<dynamic parameter specification>` one of
/// two operands, at `position`: BINARY VARYING beside a binary string,
/// else CHARACTER VARYING; none beside another parameter.
fn beside(position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
    let other = args.get(usize::from(position == 0))?.as_ref()?;
    Some(if other.is_binary() { ParameterType::Of(DataType::VarBinary { length: MAX_BINARY_LENGTH }) } else { ParameterType::Varying })
}

/// [§6.7] `<character substring function>`, and [ISO/IEC 9075-2:2023
/// §6.33] `<binary substring function>`: SUBSTRING(C FROM S \[FOR L\]),
/// called with C, S and, if written, L; [Feature T062] its positions in
/// the units written after USING, characters if none.
struct Substring {
    units: Option<CharUnits>,
}

/// [§6.7 GR1b-GR1e; ISO/IEC 9075-2:2023 §6.33 GR3, GR17] Where
/// SUBSTRING's result starts and ends in a string of `length` characters
/// or octets, from 0, end excluded, of S and L, `args` after the string;
/// none if S or L is null.
///
/// # Errors
///
/// SQLSTATE 22011 if it is of negative length [GR1d, GR6d].
fn span(args: &[Value], length: u64) -> Result<Option<(u64, u64)>> {
    let Some(s) = whole(args.get(1))? else { return Ok(None) };
    let lc = i128::from(length);
    // [GR1b] E is S + L, or the larger of LC + 1 and S.
    let e = match args.get(2) {
        Some(l) => match whole(Some(l))? {
            Some(l) => s.saturating_add(l),
            None => return Ok(None),
        },
        None => lc.saturating_add(1).max(s),
    };
    if e < s {
        return fail_with(Code::Data, SqlState::SUBSTRING_ERROR, "[ISO/IEC 9075:1992 §6.7 GR1d, GR6d] a substring of negative length");
    }
    // [GR1e] empty if it starts after C or ends before it.
    if s > lc || e < 1 {
        return Ok(Some((0, 0)));
    }
    let (s1, e1) = (s.max(1), e.min(lc.saturating_add(1)));
    Ok(Some((u64::try_from(s1.saturating_sub(1)).unwrap_or(0), u64::try_from(e1.saturating_sub(1)).unwrap_or(0))))
}

/// `n`, a position in a string held in memory, as an index.
fn index(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The length of a string held in memory, as a count.
fn length_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl Substring {
    /// [ISO/IEC 9075-2:2023 §6.33 GR3, GR17] SUBSTRING of `l`, a slice of
    /// it, by octets of a character large object if USING OCTETS is
    /// written, else by its characters.
    fn large(&self, l: &Lob, args: &[Value]) -> Result<Value> {
        let octets = self.units == Some(CharUnits::Octets) && l.kind() == LobKind::Character;
        let Some((from, to)) = span(args, if octets { l.octets() } else { l.chars() })? else { return Ok(Value::Null) };
        Ok(Value::Lob(if octets { within_octets(l, from, to)? } else { l.slice_chars(from, to.saturating_sub(from))? }))
    }
}

impl ScalarFunction for Substring {
    fn name(&self) -> &'static str {
        named(kw::SUBSTRING, self.units)
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let (source, rest) =
            args.split_first().ok_or_else(|| crate::error::SqlError::new(Code::Api, "[DESIGN] SUBSTRING needs a string"))?;
        for t in rest {
            integer(kw::SUBSTRING, t)?;
        }
        // [ISO/IEC 9075-2:2023 §6.33 SR4a ii, SR21b] of a large object, a
        // large object as long as it at most.
        if let Some(s) = large(source) {
            if s.kind == LobKind::Binary {
                no_units(kw::SUBSTRING, self.units)?;
            }
            return Ok(source.clone());
        }
        // [ISO/IEC 9075-2:2023 §6.33 SR21a] of a binary string, a varying
        // one as long as it at most.
        if let Some((length, _)) = binary_string(source) {
            no_units(kw::SUBSTRING, self.units)?;
            return Ok(DataType::VarBinary { length });
        }
        // [SR2a] varying, as long as the string at most.
        Ok(DataType::Varying { length: character(self.name(), source)?, units: CharUnits::Characters })
    }
    /// [§17.6 GR5c] the start and the length NUMERIC(P, 0), P the greatest
    /// precision; the string none \[GR4p\].
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        (position > 0).then_some(ParameterType::Of(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric }))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let source = args.first().unwrap_or(&Value::Null);
        if let Value::Lob(l) = source {
            return self.large(l, args);
        }
        if let Value::Binary(b) = source {
            let Some((from, to)) = span(args, u64::from(b.len()))? else { return Ok(Value::Null) };
            let (start, count) = (u32::try_from(from).unwrap_or(u32::MAX), u32::try_from(to.saturating_sub(from)).unwrap_or(0));
            return Ok(Value::Binary(b.substring(start.saturating_add(1), count)));
        }
        let Some(c) = text(source)? else { return Ok(Value::Null) };
        if self.units == Some(CharUnits::Octets) {
            let Some((from, to)) = span(args, length_of(c.len()))? else { return Ok(Value::Null) };
            return Ok(Value::Char(octets_within(c, index(from), index(to))));
        }
        let chars: Vec<char> = c.chars().collect();
        let Some((from, to)) = span(args, length_of(chars.len()))? else { return Ok(Value::Null) };
        Ok(Value::Char(chars.get(index(from)..index(to)).unwrap_or_default().iter().collect()))
    }
}

/// [ISO/IEC 9075-2:2023 §6.33 GR3a] The characters of `c` whose octets,
/// as UTF-8, lie wholly within octets `from` to `to`, from 0, end
/// excluded: a character cut by either end is not taken (UV074).
fn octets_within(c: &str, from: usize, to: usize) -> String {
    c.char_indices().filter(|(at, ch)| *at >= from && at.saturating_add(ch.len_utf8()) <= to).map(|(_, ch)| ch).collect()
}

/// [§6.7] `<fold>`: UPPER or LOWER, of the simple Latin letters \[GR2c\];
/// of the string's type \[SR3a\], a character large object's too [ISO/IEC
/// 9075-2:2023 §6.33 SR9].
struct Fold {
    upper: bool,
}

impl ScalarFunction for Fold {
    fn name(&self) -> &'static str {
        if self.upper { kw::UPPER } else { kw::LOWER }
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] UPPER and LOWER take one string") };
        // [SR3a] the string's type.
        if !large(s).is_some_and(|l| l.kind == LobKind::Character) {
            character(self.name(), s)?;
        }
        Ok(s.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_with(args, None)
    }
    /// [ISO/IEC 9075-2:2023 §6.33 GR7] Of a character large object, one
    /// made a chunk at a time, kept in `temp` past one.
    fn call_with(&self, args: &[Value], temp: Option<&dyn TempLobs>) -> Result<Value> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] UPPER and LOWER take one string") };
        if let Value::Lob(l) = s {
            return Ok(Value::Lob(fold(l, self.upper, temp)?));
        }
        Ok(text(s)?.map_or(Value::Null, |s| Value::Char(if self.upper { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() })))
    }
}
