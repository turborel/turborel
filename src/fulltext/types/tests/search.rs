//! Unit tests of CONTAINS and RANK: their nulls, errors, documents and
//! declarations.

use super::*;

fn contains(document: Value, pattern: &str) -> Result<Value> {
    Contains.call(&[document, text(pattern)])
}

fn integer(v: &Value) -> String {
    v.to_string()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2] CONTAINS gives 1 if the
/// document holds the pattern, else 0, of an FT_Pattern value or a
/// character string alike; 0 of a document of no characters, whatever the
/// pattern.
#[test]
fn contains_gives_one_or_zero() {
    let doc = english("The quick brown fox jumps");
    for (pattern, want) in [("\"fox\"", "1"), ("\"cat\"", "0"), ("\"fox\" & \"quick\"", "1"), ("\"cat\" | \"fox\"", "1")] {
        assert_eq!(integer(&contains(doc.clone(), pattern).unwrap()), want, "{pattern}");
        let typed = Contains.call(&[doc.clone(), ft_pattern(pattern).unwrap()]).unwrap();
        assert_eq!(integer(&typed), want, "{pattern}");
    }
    assert_eq!(integer(&contains(english(""), "\"fox\"").unwrap()), "0");
}

/// Verifies: [EXTENSION] CONTAINS of a character string or a character
/// large object reads it in FT_DefaultLanguage, as of a FullText value.
#[test]
fn contains_reads_strings_and_large_objects() {
    assert_eq!(integer(&contains(text("a red fox"), "\"fox\"").unwrap()), "1");
    let long = format!("{} needle", "hay ".repeat(100_000));
    assert_eq!(integer(&contains(Value::Lob(Lob::of_text(&long)), "\"needle\"").unwrap()), "1");
    assert_eq!(integer(&contains(Value::Lob(Lob::of_text("")), "\"needle\"").unwrap()), "0");
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2 Description 3, §5.1.3
/// Description 3] [DESIGN] a null document gives null before the pattern
/// is read, a malformed one too; a null pattern gives null; a malformed
/// pattern of a document raises 2FF01.
#[test]
fn nulls_and_malformed_patterns() {
    let rank = Rank;
    let functions: [&dyn ScalarFunction; 2] = [&Contains, &rank];
    for f in functions {
        assert!(f.call(&[Value::Null, text("\"fox\" &")]).unwrap().is_null());
        assert!(f.call(&[english("fox"), Value::Null]).unwrap().is_null());
        let e = f.call(&[english("fox"), text("\"fox\" &")]).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::INVALID_SEARCH_EXPRESSION);
        assert_eq!(f.call(&[english("fox")]).unwrap_err().code, Code::SyntaxRule);
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.3] [DESIGN] RANK by the default
/// scorer gives 1.0 to a document holding the pattern, else 0.0, and 0.0
/// to a document of no characters.
#[test]
fn rank_scores_matches_one() {
    let rank = Rank;
    assert!(matches!(rank.call(&[english("a fox"), text("\"fox\"")]).unwrap(), Value::Approx(x) if (x - 1.0).abs() < f64::EPSILON));
    assert!(matches!(rank.call(&[english("a fox"), text("\"cat\"")]).unwrap(), Value::Approx(x) if x.abs() < f64::EPSILON));
    assert!(matches!(rank.call(&[english(""), text("\"cat\"")]).unwrap(), Value::Approx(x) if x.abs() < f64::EPSILON));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2, §5.1.3] the declarations: a
/// document of FullText or a character string, a pattern of FT_Pattern or
/// a character string, and an array of thesaurus relations third; INTEGER
/// and DOUBLE PRECISION; others refused (-103). CONTAINS is no index
/// predicate but is compared with 1, its errors rechecked, its first two
/// arguments probed (RULES.md D87), and deterministic; RANK reads its
/// statement's statistics, the score an index may give greatest first.
#[test]
fn contains_and_rank_declare_their_types() {
    declare_arguments();
    let ft = FullTextType::data_type();
    let pattern = FtPatternType::data_type();
    let rank = Rank;
    for args in [[ft.clone(), pattern.clone()], [DataType::varying(9), DataType::varying(9)]] {
        assert!(matches!(Contains.return_type(&args).unwrap(), DataType::Exact { scale: 0, .. }));
        assert!(matches!(rank.return_type(&args).unwrap(), DataType::Approximate { .. }));
    }
    assert!(!Contains.index_predicate() && !rank.index_predicate());
    assert!(Contains.deterministic() && !rank.deterministic());
    assert!(rank.ranks() && rank.uses_context(2) && rank.ranked_by_index() && !Contains.ranks() && Contains.uses_context(2));
    assert_eq!(Contains.predicate_truth().map(|v| v.to_string()), Some("1".to_owned()));
    assert!(rank.predicate_truth().is_none());
    assert!(Contains.rechecked_errors() && Contains.probe_arguments() == Some(2));
}

/// The argument declarations `contains_and_rank_declare_their_types`
/// verifies.
fn declare_arguments() {
    let ft = FullTextType::data_type();
    let pattern = FtPatternType::data_type();
    assert_eq!(Contains.return_type(&[pattern.clone(), ft.clone()]).unwrap_err().code, Code::SyntaxRule);
    let appended = [ft.clone(), pattern.clone(), DataType::varying(1), DataType::varying(1), DataType::varying(1)];
    assert_eq!(Contains.return_type(&appended).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Contains.return_type(&[ft.clone(), pattern.clone(), DataType::Boolean]).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Contains.argument_type(0, &[Some(DataType::varying(4))]), Some(DataType::varying(4)));
    assert_eq!(Contains.argument_type(0, &[None]), Some(ft));
    assert_eq!(Contains.argument_type(1, &[]), Some(pattern));
    assert_eq!(Contains.argument_type(2, &[]), None);
}
