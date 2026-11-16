//! The trim and pad functions [ISO/IEC 9075-2:2023 §6.33]: TRIM of one
//! character, LTRIM, RTRIM and BTRIM of a set of them, TRIM of one octet
//! (`trim/binary.rs`), and LPAD and RPAD. A trim of a large object counts
//! what it removes from each end, reading a chunk at a time, and slices it
//! (`functions/lobs/ends.rs`, RULES.md D79).
//!
//! A trim removes characters equal to, or contained in, its trim
//! character under the collation of its source \[SR14d, SR15c\], so the
//! binder gives a trim of a source of another collation than the default
//! that collation (`binder/trim.rs`). A pad compares nothing.

use std::sync::Arc;

mod binary;

use super::ScalarFunction;
use super::lobs::ends::{leading_chars, trailing_chars};
use super::lobs::pad::{PADDED_LOB_LENGTH, padded, padding};
use super::strings::shapes::{large, shape};
use super::strings::{character, text, whole};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::value::collation::CollationRef;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind, TempLobs};
use crate::value::{CharUnits, DataType, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, Spelled, Value};

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
/// is its result's \[SR14d, SR15c, SR16e\], a regular expression
/// function's its subject \[SR8f ii, SR12f ii\]; none if it has none.
pub fn source_of(name: &str) -> Option<usize> {
    if [kw::TRIM, LTRIM, RTRIM, BTRIM, LPAD, RPAD].contains(&name) { Some(0) } else { super::regex::subject_of(name) }
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
/// S, 'BOTH', 'LEADING' or 'TRAILING' and SC if written; or LTRIM, RTRIM or
/// BTRIM (S \[, SC\]), called with S and SC. TRIM of a binary string is
/// `<binary trim function>`, of a trim octet, X'00' if none is written
/// \[SR22a\]; of a character string a space if none is \[SR14a\].
pub struct Trim {
    /// TRIM, LTRIM, RTRIM or BTRIM.
    pub name: &'static str,
    /// The collation of its source, if not the default.
    pub collation: Option<CollationRef>,
}

/// What a trim is given: its source, the ends it trims, and its trim
/// character or octet if written.
struct Operands<'v> {
    source: &'v Value,
    ends: Ends,
    trim: Option<&'v Value>,
}

impl Trim {
    /// The source and trim character or octet, if written, of `args`, of
    /// their types or values.
    fn split<'a, T>(&self, args: &'a [T]) -> Result<(&'a T, Option<&'a T>)> {
        match (self.name == kw::TRIM, args) {
            (true, [s, _]) => Ok((s, None)),
            (true, [s, _, sc]) | (false, [s, sc]) => Ok((s, Some(sc))),
            (true | false, _) => fail(Code::Api, "[DESIGN] a trim takes a string, and a set of characters if written"),
        }
    }

    /// The operands of `args`.
    fn operands<'v>(&self, args: &'v [Value]) -> Result<Operands<'v>> {
        let (source, trim) = self.split(args)?;
        let ends = if self.name == kw::TRIM {
            // [GR12e, GR18e]
            let spec = text(args.get(1).unwrap_or(&Value::Null))?.unwrap_or(kw::BOTH);
            ends_of(spec, kw::LEADING, kw::TRAILING)
        } else {
            // [GR13e, SR15b.i.2.C] BTRIM is RTRIM of LTRIM.
            ends_of(self.name, LTRIM, RTRIM)
        };
        Ok(Operands { source, ends, trim })
    }

    /// \[GR12d\] The trim character `sc`, of a TRIM one character.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22027 if it is not.
    fn single(&self, sc: &str) -> Result<()> {
        if self.name == kw::TRIM && sc.chars().count() != 1 {
            return fail_with(Code::Data, SqlState::TRIM_ERROR, "[ISO/IEC 9075:1992 §6.7 GR5d] the trim character is not one character");
        }
        Ok(())
    }

    /// \[GR12, GR13\] Of character string `source`, by `ends` and `sc`,
    /// a trim character or a set of them.
    fn characters(&self, source: &Value, ends: Ends, sc: &str) -> Result<Value> {
        self.single(sc)?;
        let set = Set::new(sc, self.collation.as_ref());
        match source {
            Value::Lob(l) => Ok(Value::Lob(trimmed_lob(l, &|c| set.contains(c), ends)?)),
            Value::Null
            | Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Extension(_) => Ok(text(source)?.map_or(Value::Null, |s| Value::Char(trimmed(s, &set, ends).to_owned()))),
        }
    }
}

/// `lob`, a character large object, with each character `trimmed` is true
/// of removed from `ends`: a slice of it.
fn trimmed_lob(lob: &Lob, trimmed: &dyn Fn(char) -> bool, ends: Ends) -> Result<Lob> {
    let n = lob.chars();
    let lead = if matches!(ends, Ends::Leading | Ends::Both) { leading_chars(lob, trimmed)? } else { 0 };
    let trail = if matches!(ends, Ends::Trailing | Ends::Both) { trailing_chars(lob, trimmed, n.saturating_sub(lead))? } else { 0 };
    lob.slice_chars(lead, n.saturating_sub(lead).saturating_sub(trail))
}

impl ScalarFunction for Trim {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let (s, sc) = self.split(args)?;
        if self.name == kw::TRIM && s.is_binary() {
            return binary_trim_type(s, sc);
        }
        // [SR14c, SR15b.i.1] the character comparable with the source.
        if let Some(sc) = sc
            && large(sc).is_none_or(|l| l.kind != LobKind::Character)
        {
            character(self.name, sc)?;
        }
        // [SR14b ii, SR15a ii] of a large object, a large object as long
        // as it at most.
        if large(s).is_some_and(|l| l.kind == LobKind::Character) {
            return Ok(s.clone());
        }
        // [SR14b i, SR15a i] varying, as long as the source at most.
        Ok(DataType::Varying { length: character(self.name, s)?, units: CharUnits::Characters })
    }
    /// \[GR12, GR13, GR18\] Null if S or SC is; S if either is empty
    /// \[GR13d\]; else S with each character or octet equal to SC, or
    /// contained in it, removed from the ends trimmed.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Operands { source, ends, trim } = self.operands(args)?;
        if source.is_null() || trim.is_some_and(Value::is_null) {
            return Ok(Value::Null);
        }
        if let Some(octet) = binary::trim_octet(source, trim)? {
            return binary::trimmed(source, octet, ends);
        }
        let sc = match trim {
            None => SPACE.to_owned(),
            Some(sc) => match set_text(sc)? {
                Some(sc) => sc,
                None => return Ok(Value::Null),
            },
        };
        self.characters(source, ends, &sc)
    }
}

/// [ISO/IEC 9075-2:2023 §6.33 SR22] The type of a `<binary trim
/// function>` of source `s`, trim octet `so` if written.
///
/// # Errors
///
/// SQLCODE -103 if the trim octet is no binary string.
fn binary_trim_type(s: &DataType, so: Option<&DataType>) -> Result<DataType> {
    if let Some(so) = so.filter(|t| !t.is_binary()) {
        return fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.33 SR22] the <trim octet> is a binary string, not {so}"));
    }
    // [SR22b ii] of a binary large object, one as long as it at most.
    if s.is_lob() {
        return Ok(s.clone());
    }
    // [SR22b i] varying, as long as the source at most.
    Ok(DataType::VarBinary { length: u32::try_from(large_or_length(s)).unwrap_or(u32::MAX) })
}

/// The length or maximum length of string type `t`.
fn large_or_length(t: &DataType) -> u64 {
    shape(t).map_or(0, |s| s.length)
}

/// The text of `sc`, a trim character or set of them: a character large
/// object's read whole, up to `MOST_SET_OCTETS`; none if null.
///
/// # Errors
///
/// SQLCODE -900 past them; -910 if it is no character string.
fn set_text(sc: &Value) -> Result<Option<String>> {
    if let Value::Lob(l) = sc {
        let most = u64::try_from(MOST_SET_OCTETS).unwrap_or(u64::MAX);
        let Some(bytes) = l.bounded(most)? else {
            return fail(Code::Unsupported, format!("[DESIGN] a trim character is a large object of more than {MOST_SET_OCTETS} octets"));
        };
        return String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| crate::value::lob::broken("a character large object's octets are not UTF-8"));
    }
    Ok(text(sc)?.map(str::to_owned))
}

/// \[DESIGN\] The most octets of a character large object a trim takes
/// as its trim characters, held in memory: one chunk's.
const MOST_SET_OCTETS: usize = CHUNK_OCTETS;

/// The trim character none is written gives \[SR14a\].
const SPACE: &str = " ";

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
        for t in [ps, pv] {
            // [SR16c, SR16d] character strings, large objects too.
            if !t.is_character() {
                let msg = format!("[ISO/IEC 9075-2:2023 §6.33 SR16] {} takes character strings, not {t}", self.name());
                return fail(Code::SyntaxRule, msg);
            }
        }
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
        // [SR16c] of a character large object, one (IL046).
        if ps.is_lob() {
            return Ok(DataType::Lob { kind: LobKind::Character, length: PADDED_LOB_LENGTH, units: CharUnits::Characters });
        }
        Ok(DataType::Varying { length: PADDED_LENGTH, units: CharUnits::Characters })
    }
    /// \[EXTENSION\] The total pad length NUMERIC(P, 0), as SUBSTRING's
    /// length [ISO/IEC 9075:1992 §17.6 GR5c]; the strings none.
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        (position == 1).then_some(ParameterType::Of(PAD_LENGTH))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_with(args, None)
    }
    /// \[GR14\] PS padded with PV, repeated and cut, to TPL characters, or
    /// cut to them; at most LR. Of an empty PV, LPV is 1 \[GR14f\], so each
    /// repetition adds nothing and the result is PS. A character large
    /// object PS is padded in `temp` (`functions/lobs/pad.rs`).
    fn call_with(&self, args: &[Value], temp: Option<&dyn TempLobs>) -> Result<Value> {
        let [ps, tpl, pv] = args else { return fail(Code::Api, "[DESIGN] a pad takes a string, a length and a string") };
        // [GR14a, GR14b]
        if ps.is_null() {
            return Ok(Value::Null);
        }
        let Some(tpl) = whole(Some(tpl))? else { return Ok(Value::Null) };
        // [GR14c]
        if tpl < 1 {
            return Ok(if let Value::Lob(_) = ps { Value::Lob(Lob::empty(LobKind::Character)) } else { Value::Char(String::new()) });
        }
        // [GR14d]
        let Some(pv) = padding(pv)? else { return Ok(Value::Null) };
        if let Value::Lob(l) = ps {
            return padded(l, u64::try_from(tpl).unwrap_or(u64::MAX), &pv, self.left, temp).map(Value::Lob);
        }
        let Some(ps) = text(ps)? else { return Ok(Value::Null) };
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
