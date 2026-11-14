//! Unit tests of the `<row pattern>` [ISO/IEC 9075-2:2023 §7.9]: each
//! primary and quantifier, and how they combine.

use super::*;
use crate::error::Code;

/// The pattern `text` is, parsed whole.
fn parsed(text: &str) -> Result<RowPattern> {
    let mut p = Parser::new(text)?;
    let pattern = p.row_pattern()?;
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the pattern"));
    }
    Ok(pattern)
}

/// `p` written back, its quantifiers in braces as SR19 reads them, a
/// reluctant one with `?` after, and each concatenation and alternation in
/// parentheses.
fn shape(p: &RowPattern) -> String {
    let list = |l: &[RowPattern], sep: &str| l.iter().map(shape).collect::<Vec<_>>().join(sep);
    match p {
        RowPattern::Variable(name, _) => name.clone(),
        RowPattern::Start => "^".into(),
        RowPattern::End => "$".into(),
        RowPattern::Empty => "()".into(),
        RowPattern::Concat(l) => format!("({})", list(l, " ")),
        RowPattern::Alternation(l) => format!("({})", list(l, " | ")),
        RowPattern::Quantified(q) => {
            let max = q.max.map(|m| m.to_string()).unwrap_or_default();
            format!("{}{{{},{max}}}{}", shape(&q.pattern), q.min, if q.reluctant { "?" } else { "" })
        }
        RowPattern::Excluded(inner) => format!("{{-{}-}}", shape(inner)),
        RowPattern::Permute(l) => format!("PERMUTE({})", list(l, ", ")),
    }
}

/// The shape of the pattern `text`, which parses.
fn shaped(text: &str) -> String {
    shape(&parsed(text).unwrap_or_else(|e| panic!("{text}: {e:?}")))
}

/// The SQLCODE parsing `text` fails with.
fn refused(text: &str) -> Option<Code> {
    parsed(text).err().map(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR19] each quantifier
/// read as its braces: `*` { 0, }, `+` { 1, }, `?` { 0, 1 }, { n } { n, n
/// }, { , m } { 0, m }, { , } { 0, }, { n, } and { n, m } as written; none
/// reluctant.
#[test]
fn each_quantifier_is_read_as_its_bounds_7_9() {
    assert_eq!(shaped("A* B+ C? D{2} E{2,} F{,3} G{1,3} H{,}"), "(A{0,} B{1,} C{0,1} D{2,2} E{2,} F{0,3} G{1,3} H{0,})");
    assert_eq!(shaped("A"), "A", "a primary of no quantifier is the primary");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR19a, c, e, f, j] a `?`
/// after a quantifier makes it reluctant, the `?` itself not read again; a
/// `?` after { n } is a syntax error; `??)` is the right bracket trigraph
/// [§5.2], so a reluctant `?` before ")" is written apart from it.
///
/// MC/DC of the reluctance. `may_be_reluctant` is shown by `D{1,2}?`,
/// reluctant, against `A{2}?`, whose `?` is left over and refused. The `?`
/// taken is shown by `A*?` against `A*`.
#[test]
fn a_question_mark_after_a_quantifier_makes_it_reluctant_7_9() {
    assert_eq!(shaped("A*? B+? C?? D{1,2}? E{,}? F{,4}?"), "(A{0,}? B{1,}? C{0,1}? D{1,2}? E{0,}? F{0,4}?)");
    assert_eq!(shaped("(A?? )"), "A{0,1}?");
    assert_eq!(shaped("A*"), "A{0,}");
    assert_eq!(refused("A{2}?"), Some(Code::Syntax));
    assert_eq!(refused("A**"), Some(Code::Syntax));
    assert!(Parser::new("(A??)").is_ok_and(|mut p| p.row_pattern().is_err()), "A then ]");
    let RowPattern::Quantified(q) = parsed("AB {2,}").unwrap() else { panic!("not quantified") };
    assert_eq!(q.at, 3, "where the quantifier begins");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR20] the least
/// repetitions of a bounded quantifier are at most its most, and its most
/// more than 0, of { n, m }, { n } and { , m } alike, else SQLCODE -103. At
/// the boundaries they are accepted.
///
/// MC/DC of `let Some(max) = bounds.max && (bounds.min > max || max ==
/// 0)`. A most written is shown by `A{3,2}`, refused, against `D{3,}`,
/// accepted. `min > max` is shown by `A{3,2}` against `A{2,2}`, and `max
/// == 0` by `A{0,0}` against `B{0,1}`.
#[test]
fn a_quantifiers_bounds_are_ordered_and_its_most_positive_7_9() {
    for wrong in ["A{3,2}", "A{0}", "A{,0}", "A{0,0}", "A{1,0}?"] {
        assert_eq!(refused(wrong), Some(Code::SyntaxRule), "{wrong}");
    }
    assert_eq!(shaped("A{2,2} B{0,1} C{0,} D{3,}"), "(A{2,2} B{0,1} C{0,} D{3,})");
    assert_eq!(shaped("A{4294967295}"), "A{4294967295,4294967295}");
}

/// Verifies: [DESIGN] a bound past `u32::MAX` is SQLCODE -900, not a
/// crash; a quantifier's bounds are unsigned integers, so anything else
/// is a syntax error.
#[test]
fn a_bound_is_an_unsigned_integer_within_the_maximum() {
    assert_eq!(refused("A{4294967296}"), Some(Code::Unsupported));
    assert_eq!(refused("A{1,4294967296}"), Some(Code::Unsupported));
    for wrong in ["A{}", "A{1.5}", "A{-1}", "A{1", "A{X}"] {
        assert!(matches!(refused(wrong), Some(Code::Syntax | Code::Lexical)), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9] concatenation binds
/// tighter than alternation, alternation of more than two terms is one
/// list, and parentheses group.
#[test]
fn concatenation_binds_tighter_than_alternation_7_9() {
    assert_eq!(shaped("A B | C D"), "((A B) | (C D))");
    assert_eq!(shaped("A | B | C"), "(A | B | C)");
    assert_eq!(shaped("A (B | C)+"), "(A (B | C){1,})");
    assert_eq!(shaped("(A B)* C"), "((A B){0,} C)");
    assert_eq!(refused("A |"), Some(Code::Syntax));
    assert_eq!(refused("| A"), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9] each `<row pattern
/// primary>`: a variable, of a delimited name too, `^`, `$`, the empty
/// pattern, an exclusion and PERMUTE, which before no "(" is a variable's
/// name.
///
/// MC/DC of `is_word(PERMUTE) && "(" next`. The first condition is shown
/// by `PERMUTE(...)` against `A (B)`, a variable and then a group. The
/// second is shown by `PERMUTE(...)` against `PERMUTE` alone.
#[test]
fn each_row_pattern_primary_7_9() {
    assert_eq!(shaped("^ A \"b\" $"), "(^ A b $)");
    assert_eq!(shaped("^A$"), "(^ A $)");
    assert_eq!(shaped("()"), "()");
    assert_eq!(shaped("A () B"), "(A () B)");
    assert_eq!(shaped("A {- B C -} D"), "(A {-(B C)-} D)");
    assert_eq!(shaped("{-A-}*"), "{-A-}{0,}");
    assert_eq!(shaped("PERMUTE(A, B C, D | E)"), "PERMUTE(A, (B C), (D | E))");
    assert_eq!(shaped("PERMUTE"), "PERMUTE");
    assert_eq!(shaped("A (B)"), "(A B)");
    let RowPattern::Concat(list) = parsed("A  B").unwrap() else { panic!("not a concatenation") };
    assert!(matches!(list.as_slice(), [RowPattern::Variable(a, 0), RowPattern::Variable(b, 3)] if a == "A" && b == "B"));
    for wrong in ["{- A", "A -}", "PERMUTE()", "1", "FIRST", "+ A"] {
        assert!(refused(wrong).is_some(), "{wrong}");
    }
}

/// Verifies: [DESIGN] a pattern nested deeper than `MAX_NESTING_DEPTH` is
/// SQLCODE -900, not a stack overflow; one within it parses.
#[test]
fn a_pattern_nested_too_deep_is_refused() {
    let deep = |n: usize| format!("{}A{}", "(".repeat(n), ")".repeat(n));
    assert_eq!(refused(&deep(crate::parser::MAX_NESTING_DEPTH)), Some(Code::Unsupported));
    assert_eq!(shaped(&deep(crate::parser::MAX_NESTING_DEPTH - 2)), "A");
    let excluded = format!("{}A{}", "{-".repeat(100), "-}".repeat(100));
    assert_eq!(refused(&excluded), Some(Code::Unsupported));
}
