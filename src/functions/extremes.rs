//! MAX and MIN, the set functions that keep the best value of a group
//! [X3.135-1986 §5.8].
//!
//! \[DESIGN\] (RULES.md D1290) Given a vector of character strings, each
//! compares the strings where they lie in the vector and makes a value of
//! the one that wins it alone (`TextOrder`).

use super::columnar::{Columnar, Kind};
use super::{Accumulator, AggregateFunction, floats, ints};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::storage::sel::nth;
use crate::storage::{Floats, Ints, Texts};
use crate::value::codec::{put_value, value};
use crate::value::collation::CollationRef;
use crate::value::{DataType, Value, compare, compare_strings};

/// \[DESIGN\] (RULES.md D1290) How MAX or MIN orders the character
/// strings it compares where they lie: whether it keeps the greatest, and
/// whether by their octets as they are, as a collation of NO PAD whose
/// sort key is the text orders them (RULES.md D1183), or with the shorter
/// padded with spaces, as a comparison with no collation does
/// [X3.135-1986 §5.11 GR5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextOrder {
    max: bool,
    unpadded: bool,
}

impl TextOrder {
    /// Whether `candidate` takes the place of `best`: only if it is the
    /// greater, or the less, so of two that compare equal the one met
    /// first is kept, as `ExtremeAcc::update` keeps it.
    pub(crate) fn better(self, candidate: &str, best: &str) -> bool {
        let order = if self.unpadded { candidate.as_bytes().cmp(best.as_bytes()) } else { compare_strings(candidate, best) };
        order == if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less }
    }

    /// The string of `texts` at `rows`, or of all, that takes the place
    /// of `held`, the best so far, and of every string of them before it;
    /// none if none betters `held`. Each is read where it lies, a null
    /// passed over, no value made.
    fn best_of<'t>(self, texts: &'t Texts, rows: Option<&[u32]>, held: Option<&str>) -> Option<&'t str> {
        let mut found: Option<&'t str> = None;
        for n in 0..rows.map_or(texts.len(), <[u32]>::len) {
            let Some(candidate) = texts.get(nth(rows, n)) else { continue };
            let against = if found.is_some() { found } else { held };
            if against.is_none_or(|best| self.better(candidate, best)) {
                found = Some(candidate);
            }
        }
        found
    }
}

/// MAX and MIN. [X3.135-1986 §5.8 SR8] the result type is the argument's. [X3.135-1986 §5.8 GR4d] by the [X3.135-1986 §5.11] rules.
pub(super) struct Extreme {
    pub(super) max: bool,
}

impl AggregateFunction for Extreme {
    fn name(&self) -> &str {
        if self.max { kw::MAX } else { kw::MIN }
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        // [X3.135-1986 §5.8 F] only COUNT has the form (*); the extension call
        // syntax always passes an argument too.
        arg.ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §5.8 F] {} takes an argument", self.name())))
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        ExtremeAcc::of(self.max, Value::Null, &result)
    }
    fn columnar(&self) -> Option<Columnar> {
        Some(Columnar(if self.max { Kind::Max } else { Kind::Min }))
    }
}

/// The best value so far; whether it keeps the greatest; whether its
/// state is written (`Accumulator::write_state`): not of a plug-in's type,
/// whose values are read back only through the types registered; and
/// whether its values are character strings held whole, not large
/// objects.
pub(super) struct ExtremeAcc {
    best: Value,
    max: bool,
    written: bool,
    strings: bool,
}

impl ExtremeAcc {
    /// MAX's accumulator, if `max`, or MIN's, of result type `result`, the
    /// best value so far `best`, null of none.
    pub(super) fn of(max: bool, best: Value, result: &DataType) -> Box<dyn Accumulator> {
        let strings = result.is_character() && !result.is_lob();
        Box::new(ExtremeAcc { best, max, written: !matches!(result, DataType::Extension(_)), strings })
    }
}

impl Accumulator for ExtremeAcc {
    /// [X3.135-1986 §5.8 GR4d] keeps the maximum or minimum by the comparison
    /// rules of [X3.135-1986 §5.11].
    fn update(&mut self, v: &Value) -> Result<()> {
        let better = match compare(v, &self.best) {
            None => true, // nothing seen yet
            Some(o) => o == if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less },
        };
        if better {
            self.best = v.clone();
        }
        Ok(())
    }
    /// The best of the vector found first, by `update`'s rule, then
    /// cloned once.
    fn update_many(&mut self, values: &[Value], rows: Option<&[u32]>) -> Result<()> {
        let wanted = if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less };
        let found = match rows {
            Some(rows) => best_of(rows.iter().filter_map(|&r| values.get(crate::storage::mask::index(r))), wanted),
            None => best_of(values.iter(), wanted),
        };
        found.map_or(Ok(()), |v| self.update(v))
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[u32]>, kernels: &dyn crate::kernels::Kernels) -> Result<()> {
        ints::extreme(ints, rows, self.max, kernels).map_or(Ok(()), |v| self.update(&v))
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[u32]>) -> Result<()> {
        floats::extreme(floats, rows, self.max).map_or(Ok(()), |v| self.update(&v))
    }
    /// \[DESIGN\] (RULES.md D1290) The best of the vector's strings found
    /// where they lie, by `update`'s rule of two strings, and a value made
    /// of it alone, if it betters the best so far. A best so far that is
    /// no string is given the values, as before.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[u32]>) -> Result<()> {
        let order = TextOrder { max: self.max, unpadded: false };
        let found = if let Value::Char(best) = &self.best {
            order.best_of(texts, rows, Some(best))
        } else if self.best.is_null() {
            order.best_of(texts, rows, None)
        } else {
            return self.update_many(texts.values(), rows);
        };
        if let Some(best) = found {
            self.best = Value::Char(best.into());
        }
        Ok(())
    }
    /// The best value so far, null if none, as `codec::put_value` writes it.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        if self.written {
            put_value(out, &self.best);
        }
        self.written
    }
    /// The best value of the state, given as `update` is given a value:
    /// so a tie keeps the one seen first, as when given each value.
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of MAX or MIN");
        let best = value(&mut reader)?;
        reader.finish()?;
        if best.is_null() { Ok(()) } else { self.update(&best) }
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.best.clone())
    }
    /// Padded, of a result that is a character string held whole.
    fn text_order(&self) -> Option<TextOrder> {
        self.strings.then_some(TextOrder { max: self.max, unpadded: false })
    }
}

/// The value of `values` not null that each one after replaces if it
/// compares with it as `wanted`, or not at all (`ExtremeAcc::update`).
fn best_of<'v>(values: impl Iterator<Item = &'v Value>, wanted: std::cmp::Ordering) -> Option<&'v Value> {
    values.filter(|v| !v.is_null()).reduce(|best, v| if compare(v, best).is_none_or(|o| o == wanted) { v } else { best })
}

/// \[EXTENSION\] MAX or MIN by a collation (RULES.md D60): the value whose
/// sort key is greatest or least [ISO/IEC 9075-2:2023 §9.15]. Under a
/// collation whose key is the text itself (`CollationRef::is_octets`) no
/// key is made: the strings' octets are compared, a prefix the less
/// [§4.3.2] (RULES.md D1183).
pub struct CollatedExtreme {
    /// Whether it keeps the greatest.
    pub max: bool,
    /// What it orders by.
    pub collation: CollationRef,
}

impl AggregateFunction for CollatedExtreme {
    fn name(&self) -> &str {
        if self.max { kw::MAX } else { kw::MIN }
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        arg.ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §5.8 F] {} takes an argument", self.name())))
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(CollatedAcc { best: None, max: self.max, octets: self.collation.is_octets(), collation: self.collation.clone() })
    }
}

/// The best value so far, with its sort key.
struct CollatedAcc {
    best: Option<(Value, Value)>,
    max: bool,
    /// Whether the collation's key is the text itself, so none is made.
    octets: bool,
    collation: CollationRef,
}

impl Accumulator for CollatedAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        if v.is_null() {
            return Ok(());
        }
        if let (true, Value::Char(text)) = (self.octets, v) {
            let order = TextOrder { max: self.max, unpadded: true };
            let better = |best: &Value| if let Value::Char(b) = best { order.better(text, b) } else { true };
            if self.best.as_ref().is_none_or(|(_, best)| better(best)) {
                self.best = Some((Value::Null, v.clone()));
            }
            return Ok(());
        }
        let wanted = if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less };
        let key = self.collation.sort_key(v)?;
        if self.best.as_ref().is_none_or(|(best, _)| compare(&key, best) == Some(wanted)) {
            self.best = Some((key, v.clone()));
        }
        Ok(())
    }
    fn update_many(&mut self, values: &[Value], rows: Option<&[u32]>) -> Result<()> {
        match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(crate::storage::mask::index(r))).try_for_each(|v| self.update(v)),
            None => values.iter().try_for_each(|v| self.update(v)),
        }
    }
    /// \[DESIGN\] (RULES.md D1290) Under a collation whose key is the
    /// text itself, the best of the vector's strings found where they lie,
    /// by their octets, and a value made of it alone, if it betters the
    /// best so far. Under any other each string's value is made, as its
    /// sort key is made of a value.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[u32]>) -> Result<()> {
        if !self.octets {
            return self.update_many(texts.values(), rows);
        }
        let order = TextOrder { max: self.max, unpadded: true };
        let held = if let Some((_, Value::Char(best))) = &self.best { Some(&**best) } else { None };
        if let Some(best) = order.best_of(texts, rows, held) {
            self.best = Some((Value::Null, Value::Char(best.into())));
        }
        Ok(())
    }
    fn update_ints(&mut self, _: &Ints, _: Option<&[u32]>, _: &dyn crate::kernels::Kernels) -> Result<()> {
        Ok(())
    }
    fn update_floats(&mut self, _: &Floats, _: Option<&[u32]>) -> Result<()> {
        Ok(())
    }
    /// Its state is written, as `ExtremeAcc`'s.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        put_value(out, self.best.as_ref().map_or(&Value::Null, |(_, v)| v));
        true
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of MAX or MIN");
        let best = value(&mut reader)?;
        reader.finish()?;
        self.update(&best)
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.best.as_ref().map_or(Value::Null, |(_, v)| v.clone()))
    }
    /// By octets, under a collation whose key is the text itself; none
    /// under one that makes sort keys.
    fn text_order(&self) -> Option<TextOrder> {
        self.octets.then_some(TextOrder { max: self.max, unpadded: true })
    }
}

// Unit tests: MAX and MIN by a collation, merged and by vector.
#[cfg(test)]
mod tests;
