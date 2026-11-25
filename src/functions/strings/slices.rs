//! \[DESIGN\] SUBSTRING over a vector of character strings (RULES.md
//! D363, D1126, `ScalarFunction::eval_batch`): of a start and a length
//! that are constants, each string's slice is copied from the vector's
//! text into the result's, no value made of a row. A dictionary's entries
//! are sliced once each, while they are no more than the rows, and the
//! rows keep their codes. Each slice is the one the call gives of the
//! string [ISO/IEC 9075:1992 §6.7 GR1], as `span` places it.
//!
//! A vector whose call could raise an error, or give a null of a string
//! not null, gives none, and its rows are then called one by one: a null
//! start or length, a negative length [§6.7 GR1d], and, counted in
//! octets, a string not of one octet a character, whose cut characters
//! the call leaves out.

use std::sync::Arc;

use super::{index, length_of, span};
use crate::error::Result;
use crate::functions::Arg;
use crate::storage::{Coded, Column, Texts};
use crate::value::{CharUnits, Value};

/// SUBSTRING, counted in `units`, of `args`, a vector of strings, a
/// start and, if written, a length: none unless the start and the length
/// are constants no row's call raises an error of.
pub(super) fn of_vector(units: Option<CharUnits>, args: &[Arg<'_>]) -> Option<Result<Column>> {
    let (Arg::Column(Column::Texts(texts)), positions) = args.split_first()? else { return None };
    // The string's place, then S and L, as `span` reads them.
    let mut given = vec![Value::Null];
    for position in positions {
        let Arg::Const(Value::Exact(d)) = position else { return None };
        // [§6.7 GR1d] A negative length raises an error, a start below 1 none.
        let negative_length = d.units < 0 && given.len() > 1;
        if d.scale != 0 || negative_length {
            return None;
        }
        given.push(Value::Exact(*d));
    }
    let octets = units == Some(CharUnits::Octets);
    let how = (given.as_slice(), octets);
    // Entries no row has are sliced for nothing, so a dictionary of more
    // entries than the rows is sliced a row at a time.
    let few = texts.coded.as_deref().filter(|coded| coded.entries.len() <= coded.codes.len());
    let made = match few {
        Some(coded) => {
            let entries = Arc::new(sliced(&coded.entries, how)?);
            Texts::coded(Coded { codes: coded.codes.clone(), entries }, texts.nulls.clone())
        }
        None => sliced(texts, how)?,
    };
    Some(Ok(Column::Texts(made)))
}

/// The slice of each string of `texts` by `positions`, the start and the
/// length and whether they count octets, a null's empty and null still:
/// none if `slice_of` gives none of one.
fn sliced(texts: &Texts, (given, octets): (&[Value], bool)) -> Option<Texts> {
    let rows = texts.len();
    let (mut text, mut ends) = (String::new(), Vec::with_capacity(rows));
    for n in 0..rows {
        if let Some(s) = texts.get(n) {
            text.push_str(slice_of(s, given, octets)?);
        }
        ends.push(text.len());
    }
    Some(Texts::new(text, ends, texts.nulls.clone()))
}

/// [ISO/IEC 9075:1992 §6.7 GR1] The slice of `s` that SUBSTRING gives of
/// the positions `given`: of a string of one octet a character its
/// octets, of another its characters, found by their places; none if the
/// positions give none, or it is counted in `octets` and is not of one
/// octet a character.
fn slice_of<'s>(s: &'s str, given: &[Value], octets: bool) -> Option<&'s str> {
    if s.is_ascii() {
        let (from, to) = span(given, length_of(s.len())).ok()??;
        return s.get(index(from)..index(to));
    }
    if octets {
        return None;
    }
    let (from, to) = span(given, length_of(s.chars().count())).ok()??;
    let octet = |place: u64| s.char_indices().nth(index(place)).map_or(s.len(), |(at, _)| at);
    s.get(octet(from)..octet(to))
}

// Unit tests: each slice against the call's, of plain strings and of a
// dictionary's (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::ScalarFunction;
    use crate::functions::strings::Substring;
    use crate::storage::Mask;
    use crate::value::Decimal;

    /// A whole number, as a position is given.
    fn whole(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// The strings `of`, the third null, as a vector.
    fn strings(of: &[&str]) -> Texts {
        let mut nulls = Mask::none(of.len());
        nulls.set(2);
        let mut text = String::new();
        let mut ends = Vec::new();
        for (n, s) in of.iter().enumerate() {
            if n != 2 {
                text.push_str(s);
            }
            ends.push(text.len());
        }
        Texts::new(text, ends, Some(nulls))
    }

    /// What the call gives of each of `texts` with `positions`.
    fn called(f: &Substring, texts: &Texts, positions: &[Value]) -> Vec<Value> {
        (0..texts.len())
            .map(|n| {
                let mut args = vec![texts.get(n).map_or(Value::Null, |s| Value::Char(s.into()))];
                args.extend(positions.iter().cloned());
                f.call(&args).unwrap()
            })
            .collect()
    }

    const STRINGS: [&str; 6] = ["13-702-694-4520", "", "null", "é", "naïve café", "ab"];

    /// Verifies: [ISO/IEC 9075:1992 §6.7 GR1] SUBSTRING over a vector
    /// gives each row what its call gives: of starts before, in and past
    /// the string, with a length and with none, of strings of one octet a
    /// character and of others, an empty one and a null (RULES.md D1126).
    #[test]
    fn a_vector_s_slices_are_its_rows_calls() {
        let texts = strings(&STRINGS);
        for units in [None, Some(CharUnits::Characters)] {
            let f = Substring { units };
            for start in [-3, 0, 1, 2, 5, 40] {
                for length in [None, Some(0), Some(2), Some(7), Some(90)] {
                    let positions: Vec<Value> = std::iter::once(start).chain(length).map(whole).collect();
                    let column = Column::Texts(texts.clone());
                    let mut args = vec![Arg::Column(&column)];
                    args.extend(positions.iter().map(Arg::Const));
                    let made = f.eval_batch(&args, texts.len()).unwrap().unwrap();
                    assert_eq!(format!("{:?}", made.values()), format!("{:?}", called(&f, &texts, &positions)), "{start} {length:?}");
                }
            }
        }
    }

    /// Verifies: [DESIGN] RULES.md D1126: a dictionary's entries are
    /// sliced once each and its rows keep their codes, while the entries
    /// are no more than the rows; of more entries each row is sliced.
    #[test]
    fn a_dictionary_s_entries_are_sliced_once() {
        let entries = Arc::new(Texts::new("13-131-2".into(), vec![4, 8], None));
        let f = Substring { units: None };
        let positions = [whole(1), whole(2)];
        for codes in [vec![1_u32, 0, 1], vec![1_u32]] {
            let column = Column::Texts(Texts::coded(Coded { codes: codes.clone(), entries: Arc::clone(&entries) }, None));
            let args = [Arg::Column(&column), Arg::Const(&positions[0]), Arg::Const(&positions[1])];
            let Column::Texts(made) = f.eval_batch(&args, codes.len()).unwrap().unwrap() else { panic!("texts") };
            assert_eq!(made.coded.is_some(), codes.len() >= 2, "codes kept of entries no more than the rows");
            let got: Vec<Option<&str>> = (0..codes.len()).map(|n| made.get(n)).collect();
            assert_eq!(got, codes.iter().map(|&c| Some(if c == 1 { "31" } else { "13" })).collect::<Vec<_>>());
        }
    }

    /// Verifies: [ISO/IEC 9075:1992 §6.7 GR1d] a vector gives none, so its
    /// rows are called one by one, of a negative length, which raises an
    /// error but of a null string; of a null position; of a position not
    /// a constant; and, counted in octets, of a string of a character of
    /// two octets, whose cut characters the call leaves out (RULES.md
    /// D1126). Counted in octets a string of one octet a character is
    /// sliced.
    #[test]
    fn a_vector_gives_none_where_a_call_may_differ() {
        let column = Column::Texts(strings(&STRINGS));
        let f = Substring { units: None };
        let (one, minus, null) = (whole(1), whole(-1), Value::Null);
        let none = |args: &[Arg<'_>]| f.eval_batch(args, STRINGS.len()).is_none();
        assert!(none(&[Arg::Column(&column), Arg::Const(&one), Arg::Const(&minus)]), "a negative length");
        assert!(none(&[Arg::Column(&column), Arg::Const(&null)]), "a null start");
        assert!(none(&[Arg::Column(&column), Arg::Const(&one), Arg::Const(&null)]), "a null length");
        assert!(none(&[Arg::Column(&column), Arg::Column(&column)]), "a start not a constant");
        assert!(none(&[Arg::Const(&one), Arg::Const(&one)]), "a string not a vector");
        assert!(!none(&[Arg::Column(&column), Arg::Const(&minus), Arg::Const(&one)]), "a negative start is none");
        let by_octets = Substring { units: Some(CharUnits::Octets) };
        assert!(by_octets.eval_batch(&[Arg::Column(&column), Arg::Const(&one)], STRINGS.len()).is_none(), "octets of two a character");
        let plain = Column::Texts(Texts::new("abcd".into(), vec![2, 4], None));
        let made = by_octets.eval_batch(&[Arg::Column(&plain), Arg::Const(&one), Arg::Const(&one)], 2).unwrap().unwrap();
        assert_eq!(format!("{:?}", made.values()), format!("{:?}", [Value::Char("a".into()), Value::Char("c".into())]));
    }
}
