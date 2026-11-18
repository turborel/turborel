//! Tests of the predicates of filters: exists, comparisons, like_regex,
//! starts with, is unknown and the three-valued connectives.

use super::*;
use crate::json::path::ast::Accessor;
use crate::json::path::eval::Identity;
use crate::json::path::eval::tests::{gives, gives_with, json};
use crate::json::path::parse_path;
use crate::value::datetime::Clocked;

/// The value, in mode `mode`, of the predicate `predicate` of the item
/// the JSON text `current` is, written in a filter of `$`.
fn truth_of(mode: &str, predicate: &str, current: &str) -> Truth {
    let path = parse_path(&format!("{mode} $ ? ({predicate})"), Clocked::default()).unwrap_or_else(|e| panic!("{predicate}: {}", e.msg));
    let Expr::Access(_, Accessor::Filter(p)) = &path.expr else { panic!("{predicate}") };
    let context = json(current);
    let identity = Identity::new(&context, &[]);
    Evaluator {
        mode: path.mode,
        zone: path.zone,
        context: &context,
        variables: &[],
        identity: &identity,
        current: Some(&context),
        last: None,
    }
    .truth(p)
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12a] exists is True
/// if its wff gives an item, False if none, Unknown of an exception.
#[test]
fn exists_tests_for_any_item() {
    assert_eq!(truth_of("lax", "exists (@.a)", r#"{"a": null}"#), Truth::True);
    assert_eq!(truth_of("lax", "exists (@.b)", r#"{"a": 1}"#), Truth::False);
    assert_eq!(truth_of("strict", "exists (@.b)", r#"{"a": 1}"#), Truth::Unknown);
    assert_eq!(truth_of("lax", "exists (1 / 0)", "0"), Truth::Unknown);
    gives("strict $[*] ? (exists (@.a))", r#"[{"a": 1}, {"b": 2}, 3]"#, r#"[{"a": 1}]"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 D III to
/// VIII] each comparison operator of comparable scalars: numbers of any
/// kind, strings, truth values.
#[test]
fn comparison_operators_of_scalars() {
    let cases = [
        ("1 == 1.0", Truth::True),
        ("1 == 2", Truth::False),
        ("1 != 2", Truth::True),
        ("1 <> 1", Truth::False),
        ("1 < 2e0", Truth::True),
        ("2 < 1", Truth::False),
        ("2 > 1", Truth::True),
        ("1 > 1", Truth::False),
        ("1 <= 1", Truth::True),
        ("2 <= 1", Truth::False),
        ("1 >= 1", Truth::True),
        ("1 >= 2", Truth::False),
        ("\"a\" < \"b\"", Truth::True),
        ("\"a\" == \"a\"", Truth::True),
        ("true > false", Truth::True),
        ("true == true", Truth::True),
    ];
    for (predicate, expected) in cases {
        for mode in ["lax", "strict"] {
            assert_eq!(truth_of(mode, predicate, "0"), expected, "{mode} {predicate}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 D]
/// character strings compare by "the Unicode codepoint collation": a
/// trailing space makes a string greater, not equal, as SQL's PAD SPACE
/// would; "Z" is below "a"; a character past the BMP is above one in it.
#[test]
fn strings_compare_by_code_point() {
    assert_eq!(truth_of("lax", "@ == \"a\"", "\"a \""), Truth::False);
    assert_eq!(truth_of("lax", "@ > \"a\"", "\"a \""), Truth::True);
    assert_eq!(truth_of("lax", "\"Z\" < \"a\"", "0"), Truth::True);
    let astral = "\u{1F600}";
    let bmp = "\u{FFFD}";
    assert_eq!(truth_of("lax", &format!("\"{astral}\" > \"{bmp}\""), "0"), Truth::True);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 D, §4.48.3]
/// the SQL/JSON null is comparable with any item: equal to itself alone,
/// unequal to any other, an array too; `<=` and `>=` of two nulls hold, `<`
/// and `>` never. MC/DC of `Equal | LessOrEqual | GreaterOrEqual` of two
/// nulls: each True, `!=`, `<` and `>` False.
#[test]
fn null_compares_with_every_item() {
    let cases = [
        ("null == null", Truth::True),
        ("null != null", Truth::False),
        ("null <= null", Truth::True),
        ("null >= null", Truth::True),
        ("null < null", Truth::False),
        ("null > null", Truth::False),
        ("null == 1", Truth::False),
        ("1 != null", Truth::True),
        ("null != 1", Truth::True),
        ("null < 1", Truth::False),
        ("1 >= null", Truth::False),
    ];
    for (predicate, expected) in cases {
        assert_eq!(truth_of("strict", predicate, "0"), expected, "{predicate}");
    }
    assert_eq!(truth_of("strict", "@ != null", "[1]"), Truth::True);
    assert_eq!(truth_of("strict", "@ == null", "{}"), Truth::False);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 D I, II,
/// E] items not comparable set ERR: a number and a string, an array or an
/// object and a scalar; with nothing found the result is Unknown.
#[test]
fn incomparable_items_make_a_comparison_unknown() {
    for mode in ["lax", "strict"] {
        assert_eq!(truth_of(mode, "1 == \"1\"", "0"), Truth::Unknown, "{mode}");
        assert_eq!(truth_of(mode, "1 < \"1\"", "0"), Truth::Unknown, "{mode}");
        assert_eq!(truth_of(mode, "@ == 1", "{}"), Truth::Unknown, "{mode}");
        assert_eq!(truth_of(mode, "@ == @", "{}"), Truth::Unknown, "{mode}");
    }
    assert_eq!(truth_of("strict", "@ == 1", "[1]"), Truth::Unknown);
    assert_eq!(truth_of("lax", "@ == 1", "[[1]]"), Truth::Unknown);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 A, C, E]
/// every pair of the operands' items is compared; lax: they are unwrapped,
/// and ERR with FOUND is True (UA073); strict: ERR is Unknown whatever is
/// found; an empty operand is False.
#[test]
fn comparisons_test_every_pair() {
    assert_eq!(truth_of("lax", "@ == 2", "[1, 2, 3]"), Truth::True);
    assert_eq!(truth_of("lax", "@ == 5", "[1, 2, 3]"), Truth::False);
    assert_eq!(truth_of("lax", "@ == \"x\"", "[1, \"x\"]"), Truth::True);
    assert_eq!(truth_of("lax", "@ == \"y\"", "[1, \"x\"]"), Truth::Unknown);
    assert_eq!(truth_of("lax", "@[*] == \"x\"", "[\"x\", 1]"), Truth::True);
    assert_eq!(truth_of("strict", "@[*] == \"x\"", "[\"x\", 1]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@[*] == \"x\"", "[1, \"x\"]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@[*] == 1", "[1, 2]"), Truth::True);
    assert_eq!(truth_of("lax", "@.none == 1", "{}"), Truth::False);
    assert_eq!(truth_of("lax", "1 == @.none", "{}"), Truth::False);
    assert_eq!(truth_of("lax", "@[*] < @[*]", "[1, 2]"), Truth::True);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 1] an
/// exception of either operand makes a comparison Unknown, strict mode's
/// structural errors too.
#[test]
fn an_exception_of_an_operand_makes_a_comparison_unknown() {
    assert_eq!(truth_of("lax", "1 / 0 == 1", "0"), Truth::Unknown);
    assert_eq!(truth_of("lax", "1 == 1 / 0", "0"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@.a == 1", "{}"), Truth::Unknown);
    assert_eq!(truth_of("lax", "(1 / 0 == 1) is unknown", "0"), Truth::True);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12d] like_regex is
/// True if some string item matches; one not a string sets ERR, as does a
/// match the matcher gives up on; the outcome table as of comparisons;
/// lax: unwrapped; an exception of the wff is Unknown.
#[test]
fn like_regex_matches_each_string() {
    assert_eq!(truth_of("lax", "@ like_regex \"^ab\"", "\"abc\""), Truth::True);
    assert_eq!(truth_of("lax", "@ like_regex \"^AB\" flag \"i\"", "\"abc\""), Truth::True);
    assert_eq!(truth_of("lax", "@ like_regex \"^b\"", "\"abc\""), Truth::False);
    assert_eq!(truth_of("lax", "@ like_regex \"b\"", "[\"x\", \"abc\"]"), Truth::True);
    assert_eq!(truth_of("lax", "@ like_regex \"b\"", "[1, \"abc\"]"), Truth::True);
    assert_eq!(truth_of("lax", "@ like_regex \"z\"", "[1, \"abc\"]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@[*] like_regex \"b\"", "[\"abc\", 1]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@ like_regex \"b\"", "[\"abc\"]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@.a like_regex \"b\"", "{}"), Truth::Unknown);
    assert_eq!(truth_of("lax", "@.a like_regex \"b\"", "{}"), Truth::False);
    let hard = "\"(a|aa)*\\\\1c\"";
    let long = format!("\"{}\"", "a".repeat(60));
    assert_eq!(truth_of("lax", &format!("@ like_regex {hard}"), &long), Truth::Unknown);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12e] starts with is
/// True if some string item begins with the initial, by characters; an
/// item not a string sets ERR; the initial is to be one character string,
/// a literal or a variable, else Unknown.
#[test]
fn starts_with_tests_each_string() {
    assert_eq!(truth_of("lax", "@ starts with \"ab\"", "\"abc\""), Truth::True);
    assert_eq!(truth_of("lax", "@ starts with \"\"", "\"abc\""), Truth::True);
    assert_eq!(truth_of("lax", "@ starts with \"b\"", "\"abc\""), Truth::False);
    assert_eq!(truth_of("lax", "@ starts with \"\u{E9}\"", "\"\u{E9}t\u{E9}\""), Truth::True);
    assert_eq!(truth_of("lax", "@ starts with \"a\"", "[1, \"abc\"]"), Truth::True);
    assert_eq!(truth_of("lax", "@ starts with \"z\"", "[1, \"abc\"]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@[*] starts with \"a\"", "[\"abc\", 1]"), Truth::Unknown);
    assert_eq!(truth_of("strict", "@.a starts with \"a\"", "{}"), Truth::Unknown);
    gives_with("lax $[*] ? (@ starts with $p)", "[\"ab\", \"b\"]", &[("p", "\"a\"")], "[\"ab\"]");
    for initial in ["1", "[\"a\", \"b\"]", "null"] {
        gives_with("lax $[*] ? ((@ starts with $p) is unknown)", "[\"ab\"]", &[("p", initial)], "[\"ab\"]");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12f, GR12g] is
/// unknown is True of Unknown alone; `!` swaps True and False, keeping
/// Unknown.
#[test]
fn is_unknown_and_negation() {
    assert_eq!(truth_of("lax", "(1 == \"1\") is unknown", "0"), Truth::True);
    assert_eq!(truth_of("lax", "(1 == 1) is unknown", "0"), Truth::False);
    assert_eq!(truth_of("lax", "(1 == 2) is unknown", "0"), Truth::False);
    assert_eq!(truth_of("lax", "!(1 == 1)", "0"), Truth::False);
    assert_eq!(truth_of("lax", "!(1 == 2)", "0"), Truth::True);
    assert_eq!(truth_of("lax", "!(1 == \"1\")", "0"), Truth::Unknown);
    assert_eq!(truth_of("lax", "!exists (@.a)", "{}"), Truth::True);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12h, GR12i] the
/// truth tables of `&&` and `||`, every pair of True, False and Unknown;
/// MC/DC of each decision: either False, then either Unknown, for `&&`;
/// either True, then either Unknown, for `||`.
#[test]
fn conjunction_and_disjunction_truth_tables() {
    let operands = [("1 == 1", Truth::True), ("1 == 2", Truth::False), ("1 == \"1\"", Truth::Unknown)];
    let and_table = [
        [Truth::True, Truth::False, Truth::Unknown],
        [Truth::False, Truth::False, Truth::False],
        [Truth::Unknown, Truth::False, Truth::Unknown],
    ];
    let or_table = [
        [Truth::True, Truth::True, Truth::True],
        [Truth::True, Truth::False, Truth::Unknown],
        [Truth::True, Truth::Unknown, Truth::Unknown],
    ];
    for (i, (a, _)) in operands.iter().enumerate() {
        for (j, (b, _)) in operands.iter().enumerate() {
            assert_eq!(truth_of("lax", &format!("{a} && {b}"), "0"), and_table[i][j], "{a} && {b}");
            assert_eq!(truth_of("lax", &format!("{a} || {b}"), "0"), or_table[i][j], "{a} || {b}");
        }
    }
    for (_, t) in operands {
        assert_eq!(not(not(t)), t);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR12c iii 2 E] the
/// outcome of ERR and FOUND: strict and ERR Unknown; else FOUND True,
/// ERR Unknown, neither False; a test settles at the first FOUND in lax
/// mode, the first ERR in strict. MC/DC of `Strict && ERR` in `truth`:
/// both, FOUND too, Unknown; lax with both True; strict with FOUND alone
/// True.
#[test]
fn the_outcome_of_err_and_found() {
    let both = Outcome { err: true, found: true };
    let err = Outcome { err: true, found: false };
    let found = Outcome { err: false, found: true };
    let neither = Outcome::default();
    assert_eq!(both.truth(Mode::Strict), Truth::Unknown);
    assert_eq!(both.truth(Mode::Lax), Truth::True);
    assert_eq!(found.truth(Mode::Strict), Truth::True);
    assert_eq!(err.truth(Mode::Lax), Truth::Unknown);
    assert_eq!(neither.truth(Mode::Lax), Truth::False);
    assert_eq!(neither.truth(Mode::Strict), Truth::False);
    assert!(found.settled(Mode::Lax) && !found.settled(Mode::Strict));
    assert!(err.settled(Mode::Strict) && !err.settled(Mode::Lax));
}
