//! Unit tests of TOKENIZE, TOKENIZEPOSITION, SEGMENTIZE, TOKENIZEANDSTEM
//! and TOKENIZEPOSITIONANDSTEM: their words, places and segments, units,
//! nulls, errors and declarations.

use super::super::{
    fulltext,
    tokenizing::{FT_MAX_ARRAY_LENGTH, Tokenizing, token_position_type},
};
use super::*;

fn tokenize(f: Tokenizing, args: &[Value]) -> Value {
    f.call(args).unwrap()
}

/// The elements of the array `v`.
fn elements(v: &Value) -> Vec<Value> {
    let Value::Array(e) = v else { panic!("{v} is no array") };
    e.to_vec()
}

/// The words of an array of FullText_Token values.
fn words(v: &Value) -> Vec<String> {
    elements(v).iter().map(ToString::to_string).collect()
}

/// The FT_TokenPosition rows of an array, each `word,position,corrval`.
fn places(v: &Value) -> Vec<String> {
    let place = |row: &Value| {
        let Value::Row(fields) = row else { panic!("{row} is no row") };
        fields.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
    };
    elements(v).iter().map(place).collect()
}

/// The texts and languages of an array of FullText values.
fn segments(v: &Value) -> Vec<String> {
    let segment = |s: &Value| {
        let (language, text) = fulltext::parts(s).unwrap();
        format!("{}:{text}", language.name())
    };
    elements(v).iter().map(segment).collect()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.4, §5.1.7] TOKENIZE gives the
/// normalized words in order, TOKENIZEANDSTEM their stems; English stop
/// words are left out, and NONE has none and stems no word.
#[test]
fn tokenize_gives_normalized_words_and_stems() {
    let doc = english("The Quick brown Foxes are RUNNING, Résumé");
    assert_eq!(words(&tokenize(Tokenizing::Tokenize, std::slice::from_ref(&doc))), ["quick", "brown", "foxes", "running", "resume"]);
    assert_eq!(words(&tokenize(Tokenizing::TokenizeAndStem, &[doc])), ["quick", "brown", "fox", "run", "resum"]);
    let none = full_text(Language::None, "The foxes").unwrap();
    assert_eq!(words(&tokenize(Tokenizing::Tokenize, std::slice::from_ref(&none))), ["the", "foxes"]);
    assert_eq!(words(&tokenize(Tokenizing::TokenizeAndStem, &[none])), ["the", "foxes"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 Description 4, 7-9, §5.1.8]
/// each word's place in WORDS counts the stop words left out, so the
/// first word's is one more than the leading stop words; CORRVAL is 0;
/// TOKENIZEPOSITIONANDSTEM gives the stems at the same places.
#[test]
fn positions_in_words_count_stop_words() {
    let doc = english("The quick fox and the dogs");
    let unit = text("WORDS");
    assert_eq!(places(&tokenize(Tokenizing::TokenizePosition, &[doc.clone(), unit])), ["quick,2,0", "fox,3,0", "dogs,6,0"]);
    assert_eq!(places(&tokenize(Tokenizing::TokenizePositionAndStem, &[doc])), ["quick,2,0", "fox,3,0", "dog,6,0"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 Description 4c] of CHARACTERS
/// a word's place is its first character's, from 1, and CORRVAL its
/// length, so `t2.position - t1.position - t1.corrVal` counts the
/// characters between two words.
#[test]
fn positions_in_characters_give_distances() {
    let got = places(&tokenize(Tokenizing::TokenizePosition, &[english("a big  dog"), text("CHARACTERS")]));
    assert_eq!(got, ["big,3,3", "dog,8,3"], "8 - 3 - 3: two spaces lie between");
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 Description 2, 4b] SENTENCE and
/// SENTENCES, PARAGRAPH and PARAGRAPHS place each word in its sentence or
/// paragraph, from 1; a unit's case and trailing spaces do not count.
#[test]
fn positions_in_sentences_and_paragraphs() {
    let doc = english("One two. Three!\n\nFour five?");
    for unit in ["SENTENCE", "sentences  "] {
        let got = places(&tokenize(Tokenizing::TokenizePosition, &[doc.clone(), text(unit)]));
        assert_eq!(got, ["one,1,0", "two,1,0", "three,2,0", "four,3,0", "five,3,0"], "{unit}");
    }
    for unit in ["PARAGRAPH", "Paragraphs"] {
        let got = places(&tokenize(Tokenizing::TokenizePosition, &[doc.clone(), text(unit)]));
        assert_eq!(got, ["one,1,0", "two,1,0", "three,1,0", "four,2,0", "five,2,0"], "{unit}");
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 Description 5, §5.1.6
/// Description 2] a unit TOKENIZEPOSITION does not take, WORD and
/// CHARACTER among them, gives null, as a null unit does; SEGMENTIZE takes
/// SENTENCE and PARAGRAPH alone.
#[test]
fn units_not_taken_give_null() {
    let doc = english("a fox");
    for unit in [text("WORD"), text("CHARACTER"), text("LINES"), Value::Null] {
        assert!(tokenize(Tokenizing::TokenizePosition, &[doc.clone(), unit.clone()]).is_null(), "{unit}");
        assert!(tokenize(Tokenizing::Segmentize, &[doc.clone(), unit]).is_null());
    }
    for unit in ["SENTENCES", "PARAGRAPHS", "WORDS", "CHARACTERS"] {
        assert!(tokenize(Tokenizing::Segmentize, &[doc.clone(), text(unit)]).is_null(), "{unit}");
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.6 Description 4] SEGMENTIZE gives
/// one FullText value per sentence or paragraph, in order, in SELF's
/// language, holding its text: its terminator and quotes kept, its
/// surrounding white space trimmed, the last running to the end.
#[test]
fn segmentize_gives_sentences_and_paragraphs() {
    let doc = full_text(Language::None, "  Hello world. \"Yes,\" he said! Bye.\n\nNext one ...").unwrap();
    let sentences = segments(&tokenize(Tokenizing::Segmentize, &[doc.clone(), text("sentence")]));
    assert_eq!(sentences, ["NONE:Hello world.", "NONE:\"Yes,\" he said!", "NONE:Bye.", "NONE:Next one ..."]);
    let paragraphs = segments(&tokenize(Tokenizing::Segmentize, &[doc, text("PARAGRAPH")]));
    assert_eq!(paragraphs, ["NONE:Hello world. \"Yes,\" he said! Bye.", "NONE:Next one ..."]);
    let ideographs = segments(&tokenize(Tokenizing::Segmentize, &[english("你好。世界。"), text("SENTENCE")]));
    assert_eq!(ideographs, ["ENGLISH:你好。", "ENGLISH:世界。"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.4 Description 2-3, §5.1.5
/// Description 3, 5a, §5.1.6 Description 3, §5.1.7 Description 2-3,
/// §5.1.8 Description 3-4] each gives null of a null SELF and an empty
/// array of a value of no characters, or of no words.
#[test]
fn nulls_and_empty_values() {
    for f in super::super::tokenizing::TOKENIZING {
        let unit = text("SENTENCE");
        let args = |doc: Value| {
            if matches!(f, Tokenizing::TokenizePosition | Tokenizing::Segmentize) { vec![doc, unit.clone()] } else { vec![doc] }
        };
        assert!(tokenize(f, &args(Value::Null)).is_null(), "{}", f.name());
        assert!(elements(&tokenize(f, &args(english("")))).is_empty(), "{}", f.name());
        assert!(elements(&tokenize(f, &args(english(" ... ")))).is_empty(), "{}", f.name());
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1 Definition, §5.1.4-§5.1.8
/// Definition, §5.2.1, §7.1] the result types: arrays of FT_MaxArrayLength
/// of character strings, of (TOKEN, POSITION, CORRVAL) rows or of FullText
/// values; a call not of a FullText value and the unit it takes is
/// refused, as a SyntaxRule error.
#[test]
fn declarations_and_signatures() {
    let ft = FullTextType::data_type();
    let unit = DataType::varying(10);
    let positions = DataType::array(token_position_type(), FT_MAX_ARRAY_LENGTH);
    assert_eq!(Tokenizing::TokenizePosition.return_type(&[ft.clone(), unit.clone()]).unwrap(), positions);
    assert_eq!(Tokenizing::TokenizePositionAndStem.return_type(std::slice::from_ref(&ft)).unwrap(), positions);
    let segments = DataType::array(ft.clone(), FT_MAX_ARRAY_LENGTH);
    assert_eq!(Tokenizing::Segmentize.return_type(&[ft.clone(), unit.clone()]).unwrap(), segments);
    for f in [Tokenizing::Tokenize, Tokenizing::TokenizeAndStem] {
        let DataType::Array(a) = f.return_type(std::slice::from_ref(&ft)).unwrap() else { panic!("no array") };
        assert!(a.element.is_character());
        assert_eq!(f.return_type(&[ft.clone(), unit.clone()]).unwrap_err().code, Code::SyntaxRule);
        assert_eq!(f.return_type(std::slice::from_ref(&unit)).unwrap_err().code, Code::SyntaxRule);
        assert_eq!(f.call(&[english("a"), text("WORDS")]).unwrap_err().code, Code::SyntaxRule);
    }
    assert_eq!(Tokenizing::Segmentize.return_type(std::slice::from_ref(&ft)).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Tokenizing::TokenizePosition.return_type(&[ft.clone(), ft]).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Tokenizing::TokenizePosition.call(&[english("a")]).unwrap_err().code, Code::SyntaxRule);
    assert!(Tokenizing::Tokenize.call(&[text("not a FullText value")]).is_err());
    assert!(Tokenizing::Tokenize.deterministic());
}
