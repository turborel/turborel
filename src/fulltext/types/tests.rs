//! Unit tests of FullText, FT_Pattern and their routines (RULES.md
//! R-TEST-1): payloads, order, keys and casts; the routines' nulls and
//! errors.

use std::cmp::Ordering;

use super::*;
use crate::bytes::Put;
use crate::error::SqlState;
use crate::fulltext::value::FULLTEXT;
use crate::value::lob::Lob;
use crate::value::types::ExtensionValue;

mod pattern_keys;
mod search;
mod tokenizing;

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

fn english(s: &str) -> Value {
    full_text(Language::English, s).unwrap()
}

fn call(f: FullTextRoutine, args: &[Value]) -> Result<Value> {
    f.call(args)
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1] [DESIGN] FullText values
/// compare by language then text, equal only of one language and one
/// text; record, value and key bytes read back to equal values, and keys
/// order as values do.
#[test]
fn fulltext_values_order_and_round_trip() {
    let mut values = vec![english("b"), english("a"), full_text(Language::None, "a").unwrap(), english(""), english("a b")];
    values.sort_by(|a, b| FULLTEXT_TYPE.compare(a, b).unwrap());
    let t = FullTextType::data_type();
    let mut keys = Vec::new();
    for v in &values {
        let record = FULLTEXT_TYPE.encode_record(v, &t).unwrap();
        assert_eq!(FULLTEXT_TYPE.compare(&FULLTEXT_TYPE.decode_record(&record, &t).unwrap(), v), Some(Ordering::Equal));
        let mut out = Vec::new();
        FULLTEXT_TYPE.put_value(v, &mut out);
        let back = FULLTEXT_TYPE.value(&mut Reader::new(&out, "test")).unwrap();
        assert_eq!(FULLTEXT_TYPE.compare(&back, v), Some(Ordering::Equal));
        let mut key = Vec::new();
        FULLTEXT_TYPE.encode_key(v, &t, &mut key).unwrap();
        keys.push(key);
    }
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys order as the values do");
    let mut rest = keys[0].as_slice();
    assert_eq!(FULLTEXT_TYPE.compare(&FULLTEXT_TYPE.decode_key(&mut rest, &t).unwrap(), &values[0]), Some(Ordering::Equal));
    assert_eq!(rest, b"");
    assert_ne!(FULLTEXT_TYPE.compare(&english("a"), &full_text(Language::None, "a").unwrap()), Some(Ordering::Equal));
    assert_eq!(FULLTEXT_TYPE.compare(&english("a"), &Value::Null), None);
}

/// Verifies: [DESIGN] a stored FullText payload is refused unless it is
/// one `encode` writes: an unknown version, a language spelled otherwise
/// or unknown, text not in UTF-8.
#[test]
fn noncanonical_payloads_are_refused() {
    let t = FullTextType::data_type();
    let good = crate::fulltext::value::encode(Language::English, "x");
    assert!(FULLTEXT_TYPE.decode_record(&good, &t).is_some());
    let mut lower = good.clone();
    lower[2] = b'e';
    let mut version = good.clone();
    version[0] = 9;
    let mut utf8 = good;
    utf8.push(0xFF);
    for bad in [lower, version, utf8, vec![1, 3, b'F', b'O', b'O']] {
        assert!(FULLTEXT_TYPE.decode_record(&bad, &t).is_none(), "{bad:?}");
        let mut out = Vec::new();
        out.put_bytes(&bad);
        assert_eq!(FULLTEXT_TYPE.value(&mut Reader::new(&out, "test")).unwrap_err().code, Code::Corrupt);
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.9, §5.1.10] CAST of a
/// character string, or a character large object, to FULLTEXT is its
/// value in ENGLISH; CAST to a character string, its text, as shown and
/// as a literal; a text longer than FT_MaxTextLength is refused, 22001.
#[test]
fn fulltext_casts_from_and_to_text() {
    let t = FullTextType::data_type();
    let v = FULLTEXT_TYPE.cast(text("It's here"), &t).unwrap();
    assert_eq!(FULLTEXT_TYPE.compare(&v, &english("It's here")), Some(Ordering::Equal));
    assert_eq!(FULLTEXT_TYPE.text(&v), "It's here");
    assert_eq!(FULLTEXT_TYPE.literal(&v), "FULLTEXT('It''s here', 'ENGLISH')");
    let lob = FULLTEXT_TYPE.cast(Value::Lob(Lob::of_text("long text")), &t).unwrap();
    assert_eq!(FULLTEXT_TYPE.text(&lob), "long text");
    assert!(FULLTEXT_TYPE.cast(Value::Null, &t).unwrap().is_null());
    assert_eq!(FULLTEXT_TYPE.cast(v.clone(), &t).unwrap().to_string(), v.to_string());
    assert_eq!(FULLTEXT_TYPE.cast(Value::Bool(true), &t).unwrap_err().code, Code::Data);
    let most = usize::try_from(FT_MAX_TEXT_LENGTH).unwrap();
    assert!(full_text(Language::English, &"é".repeat(most)).is_ok());
    let e = FULLTEXT_TYPE.cast(text(&"a".repeat(most + 1)), &t).unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
    let long = FULLTEXT_TYPE.cast(Value::Lob(Lob::of_text(&"a".repeat(most + 1))), &t).unwrap_err();
    assert_eq!(long.sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
    assert_eq!(FULLTEXT_TYPE.assign(text("x"), &t, "R").unwrap_err().code, Code::Data);
    assert!(!FULLTEXT_TYPE.assigns_text());
    assert!(FULLTEXT_TYPE.comparable(&t) && !FULLTEXT_TYPE.comparable(&FtPatternType::data_type()));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1] FT_Pattern: a character string
/// is assigned and cast to one of at most FT_MaxPatternLength characters
/// (22001 past it), whose text it shows; its payload is checked as read;
/// it compares with its own values alone.
#[test]
fn ft_pattern_is_a_character_string() {
    let t = FtPatternType::data_type();
    assert!(FT_PATTERN_TYPE.assigns_text());
    let p = FT_PATTERN_TYPE.cast(text("\"fox\" & 'x'"), &t).unwrap();
    assert_eq!(FT_PATTERN_TYPE.text(&p), "\"fox\" & 'x'");
    assert_eq!(FT_PATTERN_TYPE.literal(&p), "CAST('\"fox\" & ''x''' AS FT_PATTERN)");
    let record = FT_PATTERN_TYPE.encode_record(&p, &t).unwrap();
    assert_eq!(FT_PATTERN_TYPE.compare(&FT_PATTERN_TYPE.decode_record(&record, &t).unwrap(), &p), Some(Ordering::Equal));
    assert!(FT_PATTERN_TYPE.decode_record(&[2, b'a'], &t).is_none());
    assert!(FT_PATTERN_TYPE.decode_record(&[1, 0xFF], &t).is_none());
    assert_eq!(FT_PATTERN_TYPE.compare(&ft_pattern("a").unwrap(), &ft_pattern("b").unwrap()), Some(Ordering::Less));
    assert_eq!(FT_PATTERN_TYPE.compare(&ft_pattern("a").unwrap(), &english("a")), None);
    let most = usize::try_from(FT_MAX_PATTERN_LENGTH).unwrap();
    assert!(ft_pattern(&"a".repeat(most)).is_ok());
    assert_eq!(ft_pattern(&"a".repeat(most + 1)).unwrap_err().sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
    assert_eq!(FT_PATTERN_TYPE.cast(english("a"), &t).unwrap_err().code, Code::Data);
    assert!(FT_PATTERN_TYPE.comparable(&t) && !FT_PATTERN_TYPE.comparable(&DataType::varying(3)));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.9] FULLTEXT(string) is in
/// FT_DefaultLanguage; FULLTEXT(string, language) in the language named,
/// case and trailing spaces not counting; a null, empty or unknown
/// language raises 2FF02, though the string is null; a null string
/// otherwise gives null.
#[test]
fn the_fulltext_methods_take_a_language() {
    let f = FullTextRoutine::Construct;
    let v = call(f, &[text("Words")]).unwrap();
    assert_eq!(FULLTEXT_TYPE.literal(&v), "FULLTEXT('Words', 'ENGLISH')");
    let none = call(f, &[text("Words"), text("none  ")]).unwrap();
    assert_eq!(FULLTEXT_TYPE.literal(&none), "FULLTEXT('Words', 'NONE')");
    assert!(call(f, &[Value::Null]).unwrap().is_null());
    assert!(call(f, &[Value::Null, text("ENGLISH")]).unwrap().is_null());
    for language in [Value::Null, text(""), text("   "), text("KLINGON")] {
        for string in [text("w"), Value::Null] {
            let e = call(f, &[string, language.clone()]).unwrap_err();
            assert_eq!(e.sqlstate(), SqlState::INVALID_LANGUAGE_SPECIFICATION, "{language}");
        }
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.10] LANGUAGE(SELF) gives
/// its language's name, FULLTEXT_TO_CHARACTER(text) its text; each gives
/// null of null.
#[test]
fn observers_give_language_and_text() {
    let v = full_text(Language::None, "Some text").unwrap();
    assert_eq!(call(FullTextRoutine::Language, std::slice::from_ref(&v)).unwrap().to_string(), "NONE");
    assert_eq!(call(FullTextRoutine::ToCharacter, &[v]).unwrap().to_string(), "Some text");
    for f in [FullTextRoutine::Language, FullTextRoutine::ToCharacter] {
        assert!(call(f, &[Value::Null]).unwrap().is_null());
        assert_eq!(call(f, &[text("x")]).unwrap_err().code, Code::Api);
        assert_eq!(call(f, &[]).unwrap_err().code, Code::SyntaxRule);
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.9, §5.1.10] the routines'
/// declarations: argument and result types, other arities and types
/// refused with SQLCODE -103, each deterministic and no index predicate.
#[test]
fn routines_declare_their_types() {
    let ft = FullTextType::data_type();
    let c = FullTextRoutine::Construct;
    assert_eq!(c.return_type(&[DataType::varying(3)]).unwrap(), ft);
    assert_eq!(c.return_type(&[DataType::character(3), DataType::varying(7)]).unwrap(), ft);
    assert_eq!(c.return_type(std::slice::from_ref(&ft)).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(c.return_type(&[DataType::varying(3), DataType::varying(3), DataType::varying(3)]).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(c.argument_type(0, &[]), Some(DataType::varying(FT_MAX_TEXT_LENGTH)));
    assert_eq!(c.argument_type(1, &[]), Some(DataType::varying(MAX_CHARACTER_LENGTH)));
    let l = FullTextRoutine::Language;
    assert_eq!(l.return_type(std::slice::from_ref(&ft)).unwrap(), DataType::varying(FT_MAX_LANGUAGE_LENGTH));
    assert_eq!(FullTextRoutine::ToCharacter.return_type(std::slice::from_ref(&ft)).unwrap(), DataType::varying(FT_MAX_TEXT_LENGTH));
    assert_eq!(l.return_type(&[DataType::varying(3)]).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(l.argument_type(0, &[]), Some(ft));
    for f in ROUTINES {
        assert!(f.deterministic() && !f.index_predicate());
    }
    let names: Vec<String> = functions().iter().map(|f| f.name().to_owned()).collect();
    let tokenizing = ["TOKENIZE", "TOKENIZEPOSITION", "SEGMENTIZE", "TOKENIZEANDSTEM", "TOKENIZEPOSITIONANDSTEM"];
    let want: Vec<&str> = [FULLTEXT, FULLTEXT_TO_CHARACTER, LANGUAGE].into_iter().chain(tokenizing).chain(["CONTAINS", "RANK"]).collect();
    assert_eq!(names, want);
}

/// Verifies: [DESIGN] a value of another extension kind is no FullText
/// or FT_Pattern value: its record and key are refused (SQLCODE -910).
#[test]
fn other_kinds_are_refused() {
    let other = Value::Extension(ExtensionValue { kind: &FT_PATTERN_TYPE, bytes: Arc::from(&[1u8, b'a'][..]) });
    let t = FullTextType::data_type();
    assert_eq!(FULLTEXT_TYPE.encode_record(&other, &t).unwrap_err().code, Code::Api);
    assert_eq!(FULLTEXT_TYPE.encode_key(&other, &t, &mut Vec::new()).unwrap_err().code, Code::Api);
    assert_eq!(FT_PATTERN_TYPE.encode_record(&english("a"), &FtPatternType::data_type()).unwrap_err().code, Code::Api);
    assert!(FULLTEXT_TYPE.anchor(&other, &t).is_none());
}
