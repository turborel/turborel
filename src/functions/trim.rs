//! The trim and pad functions [ISO/IEC 9075-2:2023 §6.33]: TRIM of one
//! character, LTRIM, RTRIM and BTRIM of a set of them, and LPAD and RPAD.
//!
//! A trim removes characters equal to, or contained in, its trim
//! character under the collation of its source \[SR14d, SR15c\], so the
//! binder gives a trim of a source of another collation than the default
//! that collation (`binder/trim.rs`). A pad compares nothing.

use std::sync::Arc;

use super::ScalarFunction;
use super::strings::{character, text, whole};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::value::collation::CollationRef;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, Spelled, Value};

/// [ISO/IEC 9075-2:2023 §6.33] LTRIM's name, no key word of SQL-92.
pub const LTRIM: &str = "LTRIM";
/// [ISO/IEC 9075-2:2023 §6.33] RTRIM's name, no key word of SQL-92.
pub const RTRIM: &str = "RTRIM";
/// [ISO/IEC 9075-2:2023 §6.33] BTRIM's name, no key word of SQL-92.
pub const BTRIM: &str = "BTRIM";
/// [ISO/IEC 9075-2:2023 §6.33] LPAD's name, no key word of SQL-92.
pub const LPAD: &str = "LPAD";
/// [ISO/IEC 9075-2:2023 §6.33] RPAD's name, no key word of SQL-92.
pub const RPAD: &str = "RPAD";

/// The names of the trims of a set of characters and of the pads, read as
/// functions before "(".
pub const TRIMS_AND_PADS: [&str; 5] = [LTRIM, RTRIM, BTRIM, LPAD, RPAD];

/// The type of a total pad length a `?` takes: NUMERIC(P, 0), P the
/// greatest precision.
const PAD_LENGTH: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// Every trim and pad, to register: the trims of the default collation.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    let trim = |name| -> Arc<dyn ScalarFunction> { Arc::new(Trim { name, collation: None }) };
    vec![trim(kw::TRIM), trim(LTRIM), trim(RTRIM), trim(BTRIM), Arc::new(Padding { left: true }), Arc::new(Padding { left: false })]
}

/// Which argument of the function `name` is its source, whose collation
/// is its result's \[SR14d, SR15c, SR16e\]; none if it is no trim or pad.
pub fn source_of(name: &str) -> Option<usize> {
    if name == kw::TRIM {
        Some(2)
    } else if [LTRIM, RTRIM, BTRIM, LPAD, RPAD].contains(&name) {
        Some(0)
    } else {
        None
    }
}

/// The name of the trim `name` names; none if it names no trim.
pub fn trim_named(name: &str) -> Option<&'static str> {
    [kw::TRIM, LTRIM, RTRIM, BTRIM].into_iter().find(|t| *t == name)
}

/// Of a set of characters, those it is made of, and their sort keys under
/// a collation if one compares them.
struct Set<'a> {
    chars: &'a str,
    keyed: Option<(&'a CollationRef, Vec<Vec<u8>>)>,
}

/// `c`'s sort key under `collation`.
fn key(collation: &CollationRef, c: char) -> Vec<u8> {
    let mut written = [0; 4];
    let mut out = Vec::new();
    collation.collation.sort_key(c.encode_utf8(&mut written), collation.pad, &mut out);
    out
}

impl<'a> Set<'a> {
    fn new(chars: &'a str, collation: Option<&'a CollationRef>) -> Set<'a> {
        Set { chars, keyed: collation.map(|c| (c, chars.chars().map(|x| key(c, x)).collect())) }
    }

    /// Whether `c` is contained in it: equal to one of its characters,
    /// under its collation if one compares them.
    fn contains(&self, c: char) -> bool {
        match &self.keyed {
            None => self.chars.contains(c),
            Some((collation, keys)) => keys.contains(&key(collation, c)),
        }
    }
}

/// The ends of a string a trim removes characters from.
#[derive(Clone, Copy)]
enum Ends {
    /// Its start.
    Leading,
    /// Its end.
    Trailing,
    /// Both.
    Both,
}

/// The ends `word` names: its start if it is `leading`, its end if it is
/// `trailing`, else both.
fn ends_of(word: &str, leading: &str, trailing: &str) -> Ends {
    if word == leading {
        Ends::Leading
    } else if word == trailing {
        Ends::Trailing
    } else {
        Ends::Both
    }
}

/// `s` with each character contained in `set` removed from `ends`.
fn trimmed<'s>(s: &'s str, set: &Set<'_>, ends: Ends) -> &'s str {
    let s = if matches!(ends, Ends::Leading | Ends::Both) { s.trim_start_matches(|c| set.contains(c)) } else { s };
    if matches!(ends, Ends::Trailing | Ends::Both) { s.trim_end_matches(|c| set.contains(c)) } else { s }
}

/// [§6.33] `<trim function>`: TRIM(\[spec\] \[SC\] FROM S), called with
/// 'BOTH', 'LEADING' or 'TRAILING', SC (' ' if none is written \[SR14a\])
/// and S; or LTRIM, RTRIM or BTRIM (S \[, SC\]), called with S and SC.
pub struct Trim {
    /// TRIM, LTRIM, RTRIM or BTRIM.
    pub name: &'static str,
    /// The collation of its source, if not the default.
    pub collation: Option<CollationRef>,
}

impl Trim {
    /// The source and the trim character of `args`, and the ends it trims.
    fn operands<'v>(&self, args: &'v [Value]) -> Result<Option<(&'v str, &'v str, Ends)>> {
        if self.name == kw::TRIM {
            let [spec, sc, s] = args else { return fail(Code::Api, "[DESIGN] TRIM takes a specification, a character and a string") };
            let (Some(spec), Some(sc), Some(s)) = (text(spec)?, text(sc)?, text(s)?) else { return Ok(None) };
            // [GR12d]
            if sc.chars().count() != 1 {
                return fail_with(
                    Code::Data,
                    SqlState::TRIM_ERROR,
                    "[ISO/IEC 9075:1992 §6.7 GR5d] the trim character is not one character",
                );
            }
            // [GR12e]
            return Ok(Some((s, sc, ends_of(spec, kw::LEADING, kw::TRAILING))));
        }
        let [s, sc] = args else { return fail(Code::Api, "[DESIGN] LTRIM, RTRIM and BTRIM take a string and a set of characters") };
        let (Some(s), Some(sc)) = (text(s)?, text(sc)?) else { return Ok(None) };
        // [GR13e, SR15b.i.2.C] BTRIM is RTRIM of LTRIM.
        Ok(Some((s, sc, ends_of(self.name, LTRIM, RTRIM))))
    }
}

impl ScalarFunction for Trim {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let ((true, [_, sc, s]) | (false, [s, sc])) = (self.name == kw::TRIM, args) else {
            return fail(Code::Api, "[DESIGN] a trim takes a string and a set of characters");
        };
        // [SR14c, SR15b.i.1] the character comparable with the source.
        character(self.name, sc)?;
        // [SR14b, SR15a] varying, as long as the source at most.
        Ok(DataType::Varying { length: character(self.name, s)? })
    }
    /// \[GR12, GR13\] Null if S or SC is; S if either is empty \[GR13d\];
    /// else S with each character contained in SC removed from the ends
    /// trimmed.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some((s, sc, ends)) = self.operands(args)? else { return Ok(Value::Null) };
        let set = Set::new(sc, self.collation.as_ref());
        Ok(Value::Char(trimmed(s, &set, ends).to_owned()))
    }
}

/// [§6.33] `<pad function>`: LPAD or RPAD (PS, TPL \[, PV\]), called with
/// PS, TPL and PV (' ' if none is written \[SR16a\]).
pub struct Padding {
    /// Whether it is LPAD.
    pub left: bool,
}

/// [§6.33 SR16c] LR, the most characters a pad gives (IL046).
const PADDED_LENGTH: u32 = MAX_CHARACTER_LENGTH;

impl ScalarFunction for Padding {
    fn name(&self) -> &str {
        if self.left { LPAD } else { RPAD }
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [ps, tpl, pv] = args else { return fail(Code::Api, "[DESIGN] a pad takes a string, a length and a string") };
        character(self.name(), ps)?;
        // [SR16d]
        character(self.name(), pv)?;
        // [SR16b]
        if !matches!(tpl, DataType::Exact { scale: 0, .. }) {
            return fail(
                Code::SyntaxRule,
                format!(
                    "[ISO/IEC 9075-2:2023 §6.33 SR16b] the total pad length of {} is an exact number of scale 0, not {tpl}",
                    self.name()
                ),
            );
        }
        // [SR16c]
        Ok(DataType::Varying { length: PADDED_LENGTH })
    }
    /// \[EXTENSION\] The total pad length NUMERIC(P, 0), as SUBSTRING's
    /// length [ISO/IEC 9075:1992 §17.6 GR5c]; the strings none.
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        (position == 1).then_some(ParameterType::Of(PAD_LENGTH))
    }
    /// \[GR14\] PS padded with PV, repeated and cut, to TPL characters, or
    /// cut to them; at most LR. Of an empty PV, LPV is 1 \[GR14f\], so each
    /// repetition adds nothing and the result is PS.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [ps, tpl, pv] = args else { return fail(Code::Api, "[DESIGN] a pad takes a string, a length and a string") };
        // [GR14a, GR14b]
        let Some(ps) = text(ps)? else { return Ok(Value::Null) };
        let Some(tpl) = whole(Some(tpl))? else { return Ok(Value::Null) };
        // [GR14c]
        if tpl < 1 {
            return Ok(Value::Char(String::new()));
        }
        // [GR14d]
        let Some(pv) = text(pv)? else { return Ok(Value::Null) };
        // [GR14g] MRL, the least of TPL and LR.
        let mrl = usize::try_from(tpl.min(i128::from(PADDED_LENGTH))).unwrap_or(usize::MAX);
        let lps = ps.chars().count();
        // [GR14i, GR14j] PS, or its first MRL characters.
        if mrl <= lps {
            return Ok(Value::Char(ps.chars().take(mrl).collect()));
        }
        // [GR14k-GR14n] PI repetitions of PV, then its first PR characters.
        let pad: String = pv.chars().cycle().take(mrl.saturating_sub(lps)).collect();
        // [GR14o]
        Ok(Value::Char(if self.left { pad + ps } else { format!("{ps}{pad}") }))
    }
}
