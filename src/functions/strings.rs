//! [ISO/IEC 9075:1992 §6.6, §6.7, §6.13] The string functions and
//! concatenation, as `ScalarFunction` plugins registered under their key
//! words (concatenation under `||`), so an implementation can be replaced,
//! as a set function can. The parser reads each with its own syntax and
//! calls it with its operands in order (`parser/expression.rs`). Split from
//! functions.rs (RULES.md R-SIZE-2).
//!
//! Without VARCHAR, a string function's result has the CHARACTER type of
//! its greatest length, and its value its own length: a value is padded
//! only when stored [X3.135-1986 §8.7 GR7a].

use std::sync::Arc;

use super::ScalarFunction;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// The key word under which concatenation is registered: an operator, so no
/// call by name reaches it.
pub const CONCATENATION: &str = "||";

/// Every string function, to register.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    vec![
        Arc::new(Concatenation),
        Arc::new(Substring),
        Arc::new(Fold { upper: true }),
        Arc::new(Fold { upper: false }),
        Arc::new(Trim),
        Arc::new(Position),
        Arc::new(Length { name: kw::CHAR_LENGTH, unit: Unit::Characters }),
        Arc::new(Length { name: kw::OCTET_LENGTH, unit: Unit::Octets }),
        Arc::new(Length { name: kw::BIT_LENGTH, unit: Unit::Bits }),
    ]
}

/// The type of a length or position [§6.6 SR2, SR5]: exact numeric with
/// implementation-defined precision, `MAX_EXACT_PRECISION`, and scale 0.
const COUNTED: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0 };

/// The length of `t`, a character string type, or a Syntax Rule error.
fn character(name: &str, t: &DataType) -> Result<u32> {
    match t {
        DataType::Character { length } => Ok(*length),
        DataType::Exact { .. } | DataType::Approximate { .. } => {
            fail(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §6.7, §6.13] {name} takes a character string, not {t}"))
        }
    }
}

/// [§6.7 SR1] Whether `t` is exact numeric with scale 0.
fn integer(name: &str, t: &DataType) -> Result<()> {
    match t {
        DataType::Exact { scale: 0, .. } => Ok(()),
        DataType::Exact { .. } | DataType::Character { .. } | DataType::Approximate { .. } => fail(
            Code::SyntaxRule,
            format!("[ISO/IEC 9075:1992 §6.7 SR1] the start position and length of {name} are exact numbers of scale 0, not {t}"),
        ),
    }
}

/// The string of `v`, if it is one; none if it is null.
fn text(v: &Value) -> Result<Option<&str>> {
    match v {
        Value::Null => Ok(None),
        Value::Char(s) => Ok(Some(s)),
        Value::Exact(_) | Value::Approx(_) => fail(Code::Api, "[DESIGN] a string function was given a number"),
    }
}

/// The integer of `v`, if it is one; none if it is null.
fn whole(v: Option<&Value>) -> Result<Option<i128>> {
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
        Some(Value::Char(_) | Value::Approx(_)) => not_integer(),
    }
}

/// `n`, a count, as a value.
fn counted(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// Whether the operand other than `position` of two is not a `<dynamic
/// parameter specification>`.
fn other_known(position: usize, args: &[Option<DataType>]) -> bool {
    let other = usize::from(position == 0);
    args.get(other).is_some_and(Option::is_some)
}

/// [§6.13] `<concatenation>`: the characters of one string, then the
/// other's \[GR2\]; null if either is \[GR2a\]. Of the type of the sum of their
/// lengths \[SR2\].
struct Concatenation;

impl ScalarFunction for Concatenation {
    fn name(&self) -> &'static str {
        CONCATENATION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] || takes two operands") };
        let length = character(CONCATENATION, a)?.saturating_add(character(CONCATENATION, b)?);
        Ok(DataType::Character { length })
    }
    /// [§17.6 GR5d] CHARACTER VARYING, unless the other operand is one too
    /// \[GR4c\].
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        other_known(position, args).then_some(ParameterType::Varying)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] || takes two operands") };
        Ok(match (text(a)?, text(b)?) {
            (Some(a), Some(b)) => Value::Char(format!("{a}{b}")),
            _ => Value::Null,
        })
    }
}

/// [§6.7] `<character substring function>`: SUBSTRING(C FROM S \[FOR L\]),
/// called with C, S and, if written, L.
struct Substring;

impl ScalarFunction for Substring {
    fn name(&self) -> &'static str {
        kw::SUBSTRING
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let (source, rest) =
            args.split_first().ok_or_else(|| crate::error::SqlError::new(Code::Api, "[DESIGN] SUBSTRING needs a string"))?;
        let length = character(kw::SUBSTRING, source)?;
        for t in rest {
            integer(kw::SUBSTRING, t)?;
        }
        // [SR2a] as long as the string at most.
        Ok(DataType::Character { length })
    }
    /// [§17.6 GR5c] the start and the length NUMERIC(P, 0), P the greatest
    /// precision; the string none \[GR4p\].
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        (position > 0).then_some(ParameterType::Of(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0 }))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(c) = text(args.first().unwrap_or(&Value::Null))? else { return Ok(Value::Null) };
        let Some(s) = whole(args.get(1))? else { return Ok(Value::Null) };
        let chars: Vec<char> = c.chars().collect();
        let lc = i128::try_from(chars.len()).unwrap_or(i128::MAX);
        // [GR1b] E is S + L, or the larger of LC + 1 and S.
        let e = match args.get(2) {
            Some(l) => match whole(Some(l))? {
                Some(l) => s.saturating_add(l),
                None => return Ok(Value::Null),
            },
            None => lc.saturating_add(1).max(s),
        };
        if e < s {
            return fail_with(Code::Data, SqlState::SUBSTRING_ERROR, "[ISO/IEC 9075:1992 §6.7 GR1d] a substring of negative length");
        }
        // [GR1e] empty if it starts after C or ends before it.
        if s > lc || e < 1 {
            return Ok(Value::Char(String::new()));
        }
        let (s1, e1) = (s.max(1), e.min(lc.saturating_add(1)));
        let from = usize::try_from(s1.saturating_sub(1)).unwrap_or(0);
        let to = usize::try_from(e1.saturating_sub(1)).unwrap_or(0);
        Ok(Value::Char(chars.get(from..to).unwrap_or_default().iter().collect()))
    }
}

/// [§6.7] `<fold>`: UPPER or LOWER, of the simple Latin letters \[GR2c\];
/// of the string's type \[SR3a\].
struct Fold {
    upper: bool,
}

impl ScalarFunction for Fold {
    fn name(&self) -> &'static str {
        if self.upper { kw::UPPER } else { kw::LOWER }
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] UPPER and LOWER take one string") };
        Ok(DataType::Character { length: character(self.name(), s)? })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] UPPER and LOWER take one string") };
        Ok(text(s)?.map_or(Value::Null, |s| Value::Char(if self.upper { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() })))
    }
}

/// [§6.7] `<trim function>`: TRIM(\[spec\] \[SC\] FROM S), called with
/// 'BOTH', 'LEADING' or 'TRAILING', SC (' ' if none is written \[SR6c\]) and
/// S.
struct Trim;

impl ScalarFunction for Trim {
    fn name(&self) -> &'static str {
        kw::TRIM
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, sc, s] = args else { return fail(Code::Api, "[DESIGN] TRIM takes a specification, a character and a string") };
        // [SR6f] the character comparable with the string.
        character(kw::TRIM, sc)?;
        // [SR6e] as long as the string at most.
        Ok(DataType::Character { length: character(kw::TRIM, s)? })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [spec, sc, s] = args else { return fail(Code::Api, "[DESIGN] TRIM takes a specification, a character and a string") };
        let (Some(spec), Some(sc), Some(s)) = (text(spec)?, text(sc)?, text(s)?) else { return Ok(Value::Null) };
        let mut chars = sc.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return fail_with(Code::Data, SqlState::TRIM_ERROR, "[ISO/IEC 9075:1992 §6.7 GR5d] the trim character is not one character");
        };
        Ok(Value::Char(
            match spec {
                "LEADING" => s.trim_start_matches(c),
                "TRAILING" => s.trim_end_matches(c),
                _ => s.trim_matches(c),
            }
            .to_owned(),
        ))
    }
}

/// [§6.6] `<position expression>`: POSITION(A IN B), called with A and B:
/// 1 if A is empty, else where A first starts in B, from 1, or 0 \[GR1\].
struct Position;

impl ScalarFunction for Position {
    fn name(&self) -> &'static str {
        kw::POSITION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] POSITION takes two strings") };
        character(kw::POSITION, a)?;
        character(kw::POSITION, b)?;
        Ok(COUNTED)
    }
    /// [§17.6 GR5c] CHARACTER VARYING, unless the other string is one too
    /// \[GR4l\].
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        other_known(position, args).then_some(ParameterType::Varying)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] POSITION takes two strings") };
        let (Some(a), Some(b)) = (text(a)?, text(b)?) else { return Ok(Value::Null) };
        if a.is_empty() {
            return Ok(counted(1));
        }
        Ok(counted(b.find(a).map_or(0, |at| b.get(..at).map_or(0, |before| before.chars().count()).saturating_add(1))))
    }
}

/// What a length counts.
#[derive(Clone, Copy)]
enum Unit {
    /// Characters [§6.6 GR4].
    Characters,
    /// Octets of the string as UTF-8, its implementation-defined form of use
    /// \[GR5\].
    Octets,
    /// Bits of those octets \[GR6\].
    Bits,
}

/// [§6.6] `<length expression>`: CHAR_LENGTH (or CHARACTER_LENGTH),
/// OCTET_LENGTH or BIT_LENGTH.
struct Length {
    name: &'static str,
    unit: Unit,
}

impl ScalarFunction for Length {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] a length takes one string") };
        character(self.name, s)?;
        Ok(COUNTED)
    }
    /// [§17.6 GR5b] CHARACTER VARYING, of CHAR_LENGTH and OCTET_LENGTH;
    /// of BIT_LENGTH BIT VARYING \[GR5e\], which is not implemented, so none.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        match self.unit {
            Unit::Characters | Unit::Octets => Some(ParameterType::Varying),
            Unit::Bits => None,
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [s] = args else { return fail(Code::Api, "[DESIGN] a length takes one string") };
        let Some(s) = text(s)? else { return Ok(Value::Null) };
        Ok(counted(match self.unit {
            Unit::Characters => s.chars().count(),
            Unit::Octets => s.len(),
            Unit::Bits => s.len().saturating_mul(8),
        }))
    }
}
