//! [ISO/IEC 9075-2:2023 §6.33, §9.29] TRANSLATE_REGEX: a string with one
//! of an XQuery regular expression's matches, or all, replaced as
//! fn:replace replaces it.
//!
//! It is called with its units, CHARACTERS or OCTETS [Feature F846]; ALL
//! or OCCURRENCE; then the pattern, the flag, the subject, the
//! replacement, the start position and the occurrence, 1 under ALL. The
//! parser writes those not written as their implicit values [§6.33
//! SR12]. A subject that is a character large object is read a chunk at
//! a time, and the result made as one, the spans between matches slices of
//! the subject [§6.33 SR12f i].

use std::borrow::Cow;
use std::sync::Arc;

use super::subject::{Body, Subject, short, slice};
use super::{Cache, as_long_as, operands, parameter};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::functions::strings::{character, text, whole};
use crate::functions::{Arg, ScalarFunction};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::regex::{Ascii, Captures, Emit, LobText, Regex, Replacement, Searcher, Text, zero_length};
use crate::storage::{Column, Texts};
use crate::value::lob::{CHUNK_OCTETS, LobBuilder, LobKind, MAX_LOB_LENGTH, TempLobs};
use crate::value::{CharUnits, DataType, MAX_CHARACTER_LENGTH, Value};

/// [ISO/IEC 9075-2:2023 §6.33] TRANSLATE_REGEX, no key word of SQL-92.
pub const TRANSLATE_REGEX: &str = "TRANSLATE_REGEX";
/// Which argument of TRANSLATE_REGEX is its subject.
pub(super) const SUBJECT: usize = 4;

/// [§6.33 SR12f i 2] The maximum length of a TRANSLATE_REGEX with a
/// replacement, the longest string (IL047).
const TRANSLATED_LENGTH: u32 = MAX_CHARACTER_LENGTH;

/// \[DESIGN\] The fewest characters of a large object's span that its
/// TRANSLATE_REGEX takes as a slice, not a copy: a chunk's octets' worth
/// (RULES.md D79), so the result has a part at most for each chunk.
const SLICED_CHARS: usize = CHUNK_OCTETS;

/// TRANSLATE_REGEX ( PAT FLAG FL IN RSS WITH REP FROM SP USING CLU
/// OCCURRENCE RTO ).
#[derive(Default)]
pub(super) struct Translate {
    cache: Cache,
}

impl ScalarFunction for Translate {
    fn name(&self) -> &str {
        TRANSLATE_REGEX
    }
    /// [§6.33 SR12f] CHARACTER VARYING as long as its subject at most if
    /// the replacement is the empty string, else of the longest string; of
    /// a character large object, its type, or a large object of the
    /// longest (IL047).
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, _, pattern, flag, subject, replacement, from, occurrence] = args else {
            return fail(Code::Api, "[DESIGN] TRANSLATE_REGEX takes eight operands");
        };
        let strings = [("pattern", pattern), ("flag", flag), ("subject", subject), ("replacement", replacement)];
        operands(TRANSLATE_REGEX, &strings, &[("start position", from), ("occurrence", occurrence)])?;
        // A large object is never of length 0 [§6.1 SR17].
        let empty = !replacement.is_lob() && character(TRANSLATE_REGEX, replacement)? == 0;
        if empty {
            return as_long_as(TRANSLATE_REGEX, subject);
        }
        if subject.is_lob() {
            return Ok(DataType::Lob { kind: LobKind::Character, length: MAX_LOB_LENGTH, units: CharUnits::Characters });
        }
        Ok(DataType::Varying { length: TRANSLATED_LENGTH, units: CharUnits::Characters })
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[2, 3, 4, 5], &[6, 7])
    }
    /// \[GR9\] Null if an operand is, or the start position is not of the
    /// string; else the string with the match, or each match from the
    /// last, replaced.
    ///
    /// # Errors
    ///
    /// As `call_with`.
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_with(args, None)
    }
    /// As `call`, the result of a character large object made in `temp`
    /// if longer than a chunk.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201U of a match of no characters, 2201V of a replacement
    /// that is none [§9.29]; 22001 if the result is longer than the
    /// longest string, or large object.
    fn call_with(&self, args: &[Value], temp: Option<&dyn TempLobs>) -> Result<Value> {
        let [units, every, pattern, flag, subject, replacement, from, occurrence] = args else {
            return fail(Code::Api, "[DESIGN] TRANSLATE_REGEX takes eight operands");
        };
        let Some((regex, asked)) = self.asked([units, every, pattern, flag, replacement, from, occurrence])? else {
            return Ok(Value::Null);
        };
        asked.of(subject, (&mut regex.once(), &mut None), temp)
    }
    /// \[DESIGN\] (RULES.md D1188, D363) Over a vector of subjects in
    /// memory, every other operand a constant: the pattern compiled, its
    /// replacement read and its searcher made once for the vector, so its
    /// automaton is kept from one subject to the next. None of any other
    /// operands, each row then called.
    fn eval_batch(&self, args: &[Arg<'_>], rows: usize) -> Option<Result<Column>> {
        let [
            Arg::Const(units),
            Arg::Const(every),
            Arg::Const(pattern),
            Arg::Const(flag),
            Arg::Column(Column::Texts(subjects)),
            Arg::Const(replacement),
            Arg::Const(from),
            Arg::Const(occurrence),
        ] = args
        else {
            return None;
        };
        Some(self.of_texts([units, every, pattern, flag, replacement, from, occurrence], subjects, rows))
    }
}

/// What a TRANSLATE_REGEX is asked beside its subject, none of it null.
struct Asked<'a> {
    units: &'a Value,
    every: &'a Value,
    replacement: Cow<'a, str>,
    start: i128,
    occurrence: &'a Value,
}

impl Translate {
    /// The pattern of `operands` compiled, and what else they ask: the
    /// units, ALL or OCCURRENCE, the pattern, the flag, the replacement,
    /// the start position and the occurrence; none if the pattern, the
    /// flag, the replacement or the start position is null \[GR9b i\].
    fn asked<'a>(&self, operands: [&'a Value; 7]) -> Result<Option<(Arc<Regex>, Asked<'a>)>> {
        let [units, every, pattern, flag, replacement, from, occurrence] = operands;
        let (Some(pattern), Some(flag), Some(replacement), Some(start)) =
            (short(pattern)?, short(flag)?, short(replacement)?, whole(Some(from))?)
        else {
            return Ok(None);
        };
        let regex = self.cache.compiled(&pattern, &flag)?;
        Ok(Some((regex, Asked { units, every, replacement, start, occurrence })))
    }

    /// `eval_batch`'s work: each of the first `rows` of `subjects`
    /// translated as `operands` ask, a null of a null.
    fn of_texts(&self, operands: [&Value; 7], subjects: &Texts, rows: usize) -> Result<Column> {
        let Some((regex, asked)) = self.asked(operands)? else { return Ok(Column::Values(vec![Value::Null; rows])) };
        let (mut searcher, mut with) = (regex.searcher(), None);
        let mut made = Vec::with_capacity(rows);
        for n in 0..rows {
            let subject = subjects.get(n).map_or(Value::Null, |t| Value::Char(t.into()));
            made.push(asked.of(&subject, (&mut searcher, &mut with), None)?);
        }
        Ok(Column::Values(made))
    }
}

impl Asked<'_> {
    /// \[GR9\] `subject` translated: null if it is, or if the start
    /// position is not of it; else it with the match, or each match from
    /// the last, replaced, found by `searcher`, the replacement read into
    /// `with` at the first match met; the result of a character large
    /// object made in `temp` if longer than a chunk.
    fn of(
        &self,
        subject: &Value,
        (searcher, with): (&mut Searcher<'_>, &mut Option<Replacement>),
        temp: Option<&dyn TempLobs>,
    ) -> Result<Value> {
        let Some(counted) = Subject::new(subject, self.units)? else { return Ok(Value::Null) };
        // [GR9b ii]
        let Some(at) = counted.start(self.start)? else { return Ok(Value::Null) };
        let which = if text(self.every)? == Some(kw::ALL) {
            Which::All
        } else {
            // [GR9b iv 2 A]
            let Some(occurrence) = whole(Some(self.occurrence))? else { return Ok(Value::Null) };
            let Some(occurrence) = usize::try_from(occurrence).ok().filter(|o| *o >= 1) else { return Ok(counted.value().clone()) };
            Which::Nth(occurrence)
        };
        let mut job = Job { searcher, at, which, replacement: &self.replacement, with };
        match &counted.body {
            Body::Chars(chars) => {
                let mut out = Chars { chars, out: String::new() };
                job.run(chars.as_slice(), &mut out)?;
                within_most(out.out)
            }
            Body::Ascii(ascii) => {
                let mut out = Octets { ascii: *ascii, out: String::new() };
                job.run(ascii, &mut out)?;
                within_most(out.out)
            }
            Body::Lob(lob) => {
                let mut out = Built { text: lob, builder: LobBuilder::new(LobKind::Character, temp), chars: 0 };
                job.run(lob, &mut out)?;
                Ok(Value::Lob(out.builder.finish()?))
            }
        }
    }
}

/// Which matches are replaced.
#[derive(Clone, Copy)]
enum Which {
    /// Each \[GR9b iii\].
    All,
    /// The one of this occurrence, from 1 \[GR9b iv\].
    Nth(usize),
}

/// What a TRANSLATE_REGEX replaces, and with what.
struct Job<'a, 'r> {
    searcher: &'a mut Searcher<'r>,
    at: usize,
    which: Which,
    replacement: &'a str,
    /// The replacement read, once a match is met.
    with: &'a mut Option<Replacement>,
}

/// Where a TRANSLATE_REGEX puts what it makes, in order.
trait Out {
    /// Puts characters of the replacement.
    fn text(&mut self, t: &str) -> Result<()>;
    /// Puts characters `from` to `to` of the subject.
    fn span(&mut self, from: usize, to: usize) -> Result<()>;
}

impl Job<'_, '_> {
    /// [GR9b iii, iv 2 B] Puts on `out` `text` with the matches chosen
    /// replaced, front to back, which gives what replacing them from the
    /// last does, as they do not overlap.
    fn run<T: Text + ?Sized, O: Out>(&mut self, text: &T, out: &mut O) -> Result<()> {
        let mut done = 0_usize;
        match self.which {
            Which::All => {
                // Each match is found before it is replaced, as replacing one searches within it.
                let mut at = self.at;
                while at <= text.len() {
                    let Some(mv) = self.searcher.find_in(text, at)? else { break };
                    let (start, end) = mv.get(0).unwrap_or((at, at));
                    at = end.max(start.saturating_add(1));
                    self.replaced(text, &mv, &mut done, out)?;
                }
            }
            Which::Nth(n) => {
                if let Some(mv) = self.searcher.nth_in(text, self.at, n)? {
                    self.replaced(text, &mv, &mut done, out)?;
                }
            }
        }
        out.span(done, text.len())
    }

    /// [§9.29] Puts on `out` what is between the last match and `mv`, and
    /// `mv` replaced as fn:replace replaces that match alone, the
    /// replacement read at the first match, and `done` moved past it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201U if it matched no characters \[GR3\].
    fn replaced<T: Text + ?Sized, O: Out>(&mut self, text: &T, mv: &Captures, done: &mut usize, out: &mut O) -> Result<()> {
        let Some((start, end)) = mv.get(0).filter(|(start, end)| end > start) else {
            return Err(zero_length("a match of the pattern is of no characters"));
        };
        if self.with.is_none() {
            *self.with = Some(self.searcher.regex().replacement(self.replacement)?);
        }
        let Some(with) = self.with.as_ref() else { return Ok(()) };
        out.span(*done, start)?;
        self.searcher.replace_in(text, (start, end), with, |piece| match piece {
            Emit::Text(t) => out.text(t),
            Emit::Span(a, b) => out.span(a, b),
        })?;
        *done = end;
        Ok(())
    }
}

/// A result made as a string.
struct Chars<'a> {
    chars: &'a [char],
    out: String,
}

impl Out for Chars<'_> {
    fn text(&mut self, t: &str) -> Result<()> {
        self.out.push_str(t);
        Ok(())
    }
    fn span(&mut self, from: usize, to: usize) -> Result<()> {
        self.out.extend(self.chars.get(from..to).unwrap_or_default());
        Ok(())
    }
}

/// A result made as a string, of a subject of ASCII read where it lies.
struct Octets<'a> {
    ascii: Ascii<'a>,
    out: String,
}

impl Out for Octets<'_> {
    fn text(&mut self, t: &str) -> Result<()> {
        self.out.push_str(t);
        Ok(())
    }
    fn span(&mut self, from: usize, to: usize) -> Result<()> {
        self.out.push_str(self.ascii.span(from, to));
        Ok(())
    }
}

/// A result made as a character large object: a long span of the subject
/// taken as a slice of it, copying none of its octets, a short one copied,
/// so the result has few parts.
struct Built<'a, 's> {
    text: &'a LobText<'a>,
    builder: LobBuilder<'s>,
    chars: u64,
}

impl Built<'_, '_> {
    /// Counts `n` characters more, if the result stays within the longest
    /// large object.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22001 if it would not (IL047).
    fn count(&mut self, n: usize) -> Result<()> {
        self.chars = self.chars.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.chars <= MAX_LOB_LENGTH {
            return Ok(());
        }
        fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[ISO/IEC 9075-2:2023 §6.33 SR12f i 2 B] TRANSLATE_REGEX of a large object is longer than {MAX_LOB_LENGTH} characters"),
        )
    }
}

impl Out for Built<'_, '_> {
    fn text(&mut self, t: &str) -> Result<()> {
        self.count(t.chars().count())?;
        self.builder.push_str(t)
    }
    fn span(&mut self, from: usize, to: usize) -> Result<()> {
        let n = to.saturating_sub(from);
        self.count(n)?;
        if n >= SLICED_CHARS {
            return self.builder.push_lob(&slice(self.text.lob(), from, to)?);
        }
        let mut copied = String::new();
        for at in from..to {
            copied.extend(self.text.at(at)?);
        }
        self.builder.push_str(&copied)
    }
}

/// `out`, if no longer than the longest string.
///
/// # Errors
///
/// SQLSTATE 22001 if it is longer (IL047).
fn within_most(out: String) -> Result<Value> {
    let n = out.chars().count();
    if u32::try_from(n).is_ok_and(|n| n <= TRANSLATED_LENGTH) {
        return Ok(Value::Char(out.into()));
    }
    fail_with(
        Code::Data,
        SqlState::STRING_DATA_RIGHT_TRUNCATION,
        format!("[ISO/IEC 9075-2:2023 §6.33 SR12f i 2] TRANSLATE_REGEX of {n} characters is longer than {TRANSLATED_LENGTH}"),
    )
}

#[cfg(test)]
mod tests;
