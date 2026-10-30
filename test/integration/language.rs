//! Lexical rules, literals, value expressions, predicates and names
//! [X3.135-1986 §5.1-§5.20].

use turborel::error::Code;
use turborel::lexer::MAX_IDENTIFIER_LENGTH;
use turborel::parser::MAX_NESTING_DEPTH;
use turborel::value::MAX_EXACT_PRECISION;

use crate::common::{code, rows, setup};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2 SR10, SR11, SR13, SR15],
/// which take priority over X3.135-1986 §5.3: a key word is one in any case,
/// and a regular identifier's lower case letters are its upper case ones;
/// a reserved word is no regular identifier; a delimited identifier keeps
/// its case, is the regular identifier it spells in upper case, may be a
/// reserved word, and is a delimiter token.
#[test]
fn identifiers_and_key_words_in_any_case_5_2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "select id from t where Id = 1"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, r#"SELECT "ID" FROM "T" WHERE "ID" = 1"#), Ok(vec!["1".into()]), "\"ID\" is ID");
    assert_eq!(rows(&s, r#"SELECT"ID"FROM T WHERE ID = 1"#), Ok(vec!["1".into()]), "no separator around one");
    assert_eq!(code(&s, r#"SELECT "id" FROM T"#), Code::SyntaxRule, "\"id\" is not ID");
    assert_eq!(code(&s, "SELECT ID FROM T VALUE"), Code::Syntax, "VALUE is reserved");
    assert_eq!(rows(&s, r#"SELECT "VALUE".ID FROM T "VALUE" WHERE ID = 1"#), Ok(vec!["1".into()]), "but not delimited");
    assert_eq!(code(&s, r#"SELECT "" FROM T"#), Code::Syntax, "a delimited identifier has a part");
    assert_eq!(code(&s, "SELECT \"I\nD\" FROM T"), Code::Lexical, "no newline in it");
    assert_eq!(code(&s, r#"SELECT "ID FROM T"#), Code::Lexical, "unterminated");
    assert_eq!(rows(&s, r#"SELECT "A""B".ID FROM T "A""B" WHERE ID = 1"#), Ok(vec!["1".into()]), "\"\" is one quote");
}

/// Verifies: [X3.135-1986 §5.3] a tab is not a separator.
#[test]
fn a_tab_is_not_a_space_5_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T\tWHERE ID = 1"), Code::Lexical); // tab is not a <space>
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2], which takes priority
/// over X3.135-1986 §5.3: after its first letter, a regular identifier has
/// underscores anywhere, together or last; an underscore first is an
/// `<introducer>` [§5.3, §5.4], so `_A` alone, before FROM, is a syntax
/// error.
#[test]
fn underscores_go_anywhere_after_the_first_letter_5_2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT A__B_.ID FROM T A__B_ WHERE A__B_.ID = 1"), Ok(vec!["1".into()]));
    assert_eq!(code(&s, "SELECT _A FROM T"), Code::Syntax, "not first: an introducer, of no identifier");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2 SR8, SR9], which take
/// priority over X3.135-1986 §5.3 SR3: a regular identifier has at most 128
/// letters and digits, its underscores not counted, and a delimited one 128
/// parts; one more is refused.
#[test]
fn an_identifier_has_at_most_128_characters_5_2() {
    let db = setup();
    let s = db.session("APP");
    let longest = "A".repeat(MAX_IDENTIFIER_LENGTH);
    assert_eq!(MAX_IDENTIFIER_LENGTH, 128);
    assert_eq!(rows(&s, &format!("SELECT {longest}.ID FROM T {longest} WHERE ID = 1")), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, &format!("SELECT ID FROM T {longest}_ WHERE ID = 1")), Ok(vec!["1".into()]), "_ is not counted");
    assert_eq!(code(&s, &format!("SELECT {longest}B FROM T")), Code::SyntaxRule);
    assert_eq!(code(&s, &format!("SELECT \"{longest}B\" FROM T")), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §5.3 SR2] an approximate literal with no exponent digits is rejected.
#[test]
fn an_exponent_needs_digits_5_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = 1.5E"), Code::Lexical); // SR2
}

/// Verifies: [X3.135-1986 §5.3 SR2] "A <nondelimiter token> shall be
/// followed by a <delimiter token> or a <separator>": an exact or
/// approximate literal, a key word or an identifier run into a letter is
/// refused (SQLCODE of a lexical error), not read as two tokens; the same
/// tokens apart by a space, or followed by a delimiter token, are taken
/// ([EXTENSION] [ISO/IEC 9075:1992 §5.2 SR5] says the same).
#[test]
fn a_nondelimiter_token_is_followed_by_a_delimiter_or_separator_5_3() {
    let db = setup();
    let s = db.session("APP");
    for glued in ["SELECT 1A FROM T", "SELECT 1.5E2A FROM T", "SELECT ID FROM T WHERE ID = 1AND ID = 1", "SELECT 1 FROM T WHERE ID = .5X"] {
        assert_eq!(code(&s, glued), Code::Lexical, "{glued}");
    }
    assert_eq!(rows(&s, "SELECT 1 A FROM T WHERE ID = 1"), Ok(vec!["1".into()]), "a <separator> between");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID=1"), Ok(vec!["1".into()]), "a <delimiter token> after");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1\nAND ID = 1"), Ok(vec!["1".into()]), "a <newline> after");
}

/// Verifies: [X3.135-1986 §5.3]; [DESIGN] the implementor-defined end-of-line
/// indicator is `\n` or `\r\n` (CONFORMANCE.md). A comment must end with one.
#[test]
fn a_comment_must_end_with_a_newline_5_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T -- no newline"), Code::Lexical);
    assert_eq!(rows(&s, "SELECT ID FROM T -- a comment\n WHERE ID=1"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T -- a comment\r\n WHERE ID=1"), Ok(vec!["1".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR1, SR4, SR9] a
/// character string literal may be empty, which SQL-86 does not allow
/// [X3.135-1986 §5.2], and goes on in another quoted part after a
/// separator with a newline, not after one without; it must be closed.
#[test]
fn string_literals_5_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME || '' = 'ANN'"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT CHAR_LENGTH('') FROM T WHERE ID = 1"), Ok(vec!["0".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME = 'AN'\n  -- the rest\n 'N'"), Ok(vec!["1".into()]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME = 'AN' 'N'"), Code::Syntax, "no newline between");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME = 'ABC"), Code::Lexical); // unterminated literal
}

/// Verifies: [X3.135-1986 §5.1 SR1] a newline is not a character, so a string literal cannot hold one.
#[test]
fn a_newline_is_not_a_character_5_1() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME = 'A\nB'"), Code::Lexical); // §5.1 SR1: a <newline> is not a <character>
}

/// Verifies: [X3.135-1986 §5.3] "!=" is not an SQL token.
#[test]
fn not_equals_is_written_with_angle_brackets_5_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID != 1"), Code::Lexical); // "!=" is not SQL
}

/// Verifies: [X3.135-1986 §5.2 SR2, SR4, SR5] string, exact and approximate literals give their values;
/// [EXTENSION] [ISO/IEC 9075:1992 §5.1 GR1] an exponent's E in either case, as B, X and N are.
#[test]
fn literals_give_their_values_5_2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT 'IT''S', .5, 5., 1.5E2, -2.0E-1 FROM T WHERE ID = 1"), Ok(vec!["IT'S,0.5,5,1.5E2,-2E-1".into()]));
    assert_eq!(rows(&s, "SELECT 1.5e2, 2e-1 FROM T WHERE ID = 1"), Ok(vec!["1.5E2,2E-1".into()]));
}

/// Verifies: [X3.135-1986 §5.2 SR4]; [DESIGN] an exact literal of the maximum
/// precision is accepted, and one digit more gets SQLCODE -900.
#[test]
fn an_exact_literal_beyond_the_maximum_precision_is_unsupported_5_2() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.2 SR4, SR5] set no limit on digits or range; beyond this
    // implementation's, a literal is valid SQL-86 it does not process.
    let nines = |n: u32| "9".repeat(usize::try_from(n).unwrap());
    let longest = nines(MAX_EXACT_PRECISION);
    assert_eq!(rows(&s, &format!("SELECT {longest} FROM T WHERE ID = 1")), Ok(vec![longest.clone()]));
    assert_eq!(code(&s, &format!("SELECT {} FROM T", nines(MAX_EXACT_PRECISION + 1))), Code::Unsupported);
}

/// Verifies: [X3.135-1986 §5.2 SR5]; [DESIGN] an approximate literal outside
/// the range of `f64` gets SQLCODE -900.
#[test]
fn an_approximate_literal_out_of_range_is_unsupported_5_2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT 1E999 FROM T"), Code::Unsupported);
}

/// Verifies: [X3.135-1986 §5.9 SR4b, SR4c, GR7] scales of sums and products,
/// precedence, and left-to-right evaluation.
#[test]
fn sum_and_product_scales_and_precedence_5_9() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.9 SR4b/c] scales of sums and products; [X3.135-1986 §5.9 GR7] precedence and left-to-right.
    assert_eq!(rows(&s, "SELECT AMT + 1, AMT * 1.5, 10 - 4 - 3, 2 + 3 * 4 FROM T WHERE ID = 1"), Ok(vec!["11.50,15.750,3,14".into()]));
}

/// Verifies: [X3.135-1986 §5.9 SR2] a sign touching a number belongs to the
/// literal, and a monadic operator may not be followed by another sign.
#[test]
fn a_sign_after_a_monadic_operator_5_9() {
    let db = setup();
    let s = db.session("APP");
    // A sign touching a number belongs to the literal; "- 1" is monadic minus.
    assert_eq!(rows(&s, "SELECT 2 - -1, - 1 FROM T WHERE ID = 1"), Ok(vec!["3,-1".into()]));
    assert_eq!(code(&s, "SELECT - -ID FROM T"), Code::SyntaxRule); // [X3.135-1986 §5.9 SR2]
}

/// Verifies: [X3.135-1986 §5.9 SR3] a character string operand takes no operators.
#[test]
fn character_strings_take_no_operators_5_9() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT -NAME FROM T"), Code::SyntaxRule); // [X3.135-1986 §5.9 SR3]
    assert_eq!(code(&s, "SELECT NAME + 'X' FROM T"), Code::SyntaxRule); // [X3.135-1986 §5.9 SR3]
}

/// Verifies: [X3.135-1986 §5.9 GR1] a null operand gives a null result.
#[test]
fn a_null_operand_gives_null_5_9() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.9 GR1] null in, null out.
    assert_eq!(rows(&s, "SELECT AMT + 1 FROM T WHERE ID = 3"), Ok(vec!["NULL".into()]));
}

/// Verifies: [X3.135-1986 §5.9 SR5] any approximate operand makes the result approximate.
#[test]
fn an_approximate_operand_makes_the_result_approximate_5_9() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.9 SR5] any approximate operand makes the result approximate.
    assert_eq!(rows(&s, "SELECT ID + 0.5E0 FROM T WHERE ID = 1"), Ok(vec!["1.5E0".into()]));
}

/// Verifies: [X3.135-1986 §5.9 GR5] division by zero, exact or approximate, is a data exception.
#[test]
fn division_by_zero_is_a_data_exception_5_9() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID / 0 FROM T"), Code::Data); // [X3.135-1986 §5.9 GR5]
    assert_eq!(code(&s, "SELECT F / 0E0 FROM T WHERE ID = 1"), Code::Data); // GR5, approximate too
}

/// Verifies: [DESIGN] an approximate result outside the range of `f64` is a data exception.
#[test]
fn approximate_overflow_is_a_data_exception_5_9() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT 1E300 * 1E300 FROM T WHERE ID = 1"), Code::Data);
}

/// Verifies: [X3.135-1986 §5.11 GR5] strings compare as if the shorter were padded with spaces.
#[test]
fn strings_compare_as_if_padded_with_spaces_5_11() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.11 GR5] strings compare as if padded with spaces.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME = 'ANN   '"), Ok(vec!["1".into()]));
}

/// Verifies: [X3.135-1986 §5.11 GR2], [X3.135-1986 §5.18 GR2] a comparison with
/// null is unknown, NOT unknown is unknown, and OR follows the truth table.
#[test]
fn a_comparison_with_null_is_unknown_5_11() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.11 GR2] comparisons with null are unknown, and NOT unknown is unknown.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT NAME = 'ANN'"), Ok(vec!["2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME = 'ANN' OR AMT > 0"), Ok(vec!["1".into()]));
}

/// Verifies: [X3.135-1986 §5.15] IS NULL and IS NOT NULL are never unknown.
#[test]
fn is_null_is_never_unknown_5_15() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.15] IS NULL is never unknown.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME IS NULL"), Ok(vec!["3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT NAME IS NOT NULL"), Ok(vec!["3".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.6 GR2, GR3, Table 12] any
/// row value constructor may come before IS NULL, which SQL-86 allows only
/// a column [X3.135-1986 §5.15 F]: a row IS NULL if all of it is null, IS
/// NOT NULL if none is, so a row partly null is neither.
#[test]
fn rows_may_precede_is_null_8_6() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (NAME) IS NULL"), Ok(vec!["3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT + 1 IS NULL"), Ok(vec!["3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (NAME, AMT) IS NULL"), Ok(vec!["3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, NAME) IS NULL"), Ok(vec![]), "partly null");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, NAME) IS NOT NULL ORDER BY ID"), Ok(vec!["1".into(), "2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT (ID, NAME) IS NOT NULL"), Ok(vec!["3".into()]));
}

/// Verifies: [X3.135-1986 §5.11 SR1, §4.2] strings and numbers are not comparable.
#[test]
fn strings_and_numbers_are_not_comparable_5_11() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.11 SR1, §4.2] strings and numbers are not comparable.
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME = 1"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §5.11 GR4] numbers compare by algebraic value, exact
/// against approximate too.
#[test]
fn numbers_compare_by_algebraic_value_5_11() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.11 GR4] numbers by algebraic value, exact against approximate too.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT = 10.5 AND F = 1.5 AND ID = 1E0"), Ok(vec!["1".into()]));
}

/// Verifies: [X3.135-1986 §5.18 F]
#[test]
fn boolean_primary_backtracking_5_18() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID) = 1"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ((ID + 1) = 2)"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ((ID = 1 OR ID = 2)) AND NOT (ID = 1)"), Ok(vec!["2".into()]));
    // Robustness: a <search condition> is made of <predicate>s, and each operator needs both operands.
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID"), Code::Syntax);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = 1 AND"), Code::Syntax);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (ID = 1"), Code::Syntax);
}

/// Verifies: [X3.135-1986 §5.7] a column may be qualified by a table name or a correlation name.
#[test]
fn qualified_column_names_resolve_5_7() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT APP.T.ID FROM APP.T WHERE T.ID = 1"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT X.ID FROM T X WHERE X.ID = 2"), Ok(vec!["2".into()]));
}

/// Verifies: [X3.135-1986 §5.7] a table given a correlation name cannot be named by its table name.
#[test]
fn a_correlation_name_hides_the_table_name_5_7() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT T.ID FROM T X"), Code::SyntaxRule); // T is not exposed
}

/// Verifies: [X3.135-1986 §5.7] an unqualified column name must not be ambiguous.
#[test]
fn an_unqualified_column_must_be_unambiguous_5_7() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T, U"), Code::SyntaxRule); // ambiguous
}

/// Verifies: [X3.135-1986 §5.20 SR2] a table name may be exposed only once in a FROM clause.
#[test]
fn exposed_table_names_must_differ_5_20() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T, T"), Code::SyntaxRule); // [X3.135-1986 §5.20 SR2]
}

/// Verifies: [X3.135-1986 §5.20 SR3] a correlation name must differ from every
/// exposed table name, but may equal one that is not exposed.
#[test]
fn a_correlation_name_must_differ_from_exposed_table_names_5_20() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT T.NAME FROM T, U T"), Code::SyntaxRule); // [X3.135-1986 §5.20 SR3]
    assert_eq!(rows(&s, "SELECT X.ID FROM T X, U T").map(|r| r.len()), Ok(6)); // T not exposed, so legal
}

/// Verifies: [X3.135-1986 §5.20] a table may be joined with itself under two correlation names.
#[test]
fn a_table_joins_with_itself_under_correlation_names_5_20() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT X.ID, Y.ID FROM T X, T Y WHERE X.ID = 1 AND Y.ID < 3 ORDER BY 2"), Ok(vec!["1,1".into(), "1,2".into()]));
}

/// Verifies: [X3.135-1986 §5.4 SR5] a table name must identify a table in the schema.
#[test]
fn a_table_name_must_identify_a_table_5_4() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM NOPE"), Code::SyntaxRule); // [X3.135-1986 §5.4 SR5]
}

/// Verifies: [X3.135-1986 §5.11 GR4] an exact and an approximate number compare
/// by algebraic value, not after rounding the exact one to approximate.
///
/// Regression test: 2^53 + 1 compared
/// equal to 2^53 written as an approximate literal.
#[test]
fn exact_and_approximate_numbers_compare_by_algebraic_value() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1 AND 9007199254740993 = 9007199254740992E0"), Ok(vec![]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1 AND 9007199254740993 > 9007199254740992E0"), Ok(vec!["1".to_owned()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1 AND 9007199254740992 = 9007199254740992E0"), Ok(vec!["1".to_owned()]));
}

/// Verifies: [X3.135-1986 §5.9 SR4c, §5.5 SR4] a product's scale is s1 + s2. Up
/// to the maximum precision it is computed exactly; beyond it no type exists in
/// this implementation, so the statement gets SQLCODE -900 when it is bound.
///
/// Regression test: the scale was capped
/// at bind time, and every row then failed at run time, even 0 * 0.
#[test]
fn product_scale_is_the_sum_of_the_scales() {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION P CREATE TABLE P (A NUMERIC(18,9), B NUMERIC(18,10))").unwrap();
    let s = db.session("P");
    s.run("INSERT INTO P VALUES (0.000000001, 0)").unwrap();
    assert_eq!(rows(&s, "SELECT A * A FROM P"), Ok(vec!["0.000000000000000001".to_owned()]));
    assert_eq!(code(&s, "SELECT A * B FROM P"), Code::Unsupported);
    assert_eq!(code(&s, "SELECT B * B FROM P"), Code::Unsupported);
}

/// Stack for the nesting test: half of Rust's default 2 MiB thread stack, so the
/// deepest accepted statement must run with a margin of two.
const HALF_DEFAULT_STACK: usize = 1024 * 1024;
/// Far beyond the limit: the depth that used to overflow the stack.
const VERY_DEEP: usize = 100_000;

/// Builds a statement whose syntax tree is about `n` levels deep.
type NestedSql = fn(usize) -> String;

/// Statement shapes whose syntax tree grows one level per `n`: nesting and chains.
fn nesting_shapes() -> Vec<(&'static str, NestedSql)> {
    vec![
        ("value parentheses", |n| format!("SELECT {}ID{} FROM T", "(".repeat(n), ")".repeat(n))),
        ("condition parentheses", |n| format!("SELECT ID FROM T WHERE {}ID = 1{}", "(".repeat(n), ")".repeat(n))),
        ("AND chain", |n| format!("SELECT ID FROM T WHERE ID = 1{}", " AND ID = 1".repeat(n))),
        ("OR chain", |n| format!("SELECT ID FROM T WHERE ID = 2{} OR ID = 1", " OR ID = 2".repeat(n))),
        ("+ chain", |n| format!("SELECT ID{} FROM T", " + 1".repeat(n))),
        ("* chain", |n| format!("SELECT ID{} FROM T", " * 1".repeat(n))),
        ("UNION chain", |n| format!("SELECT ID FROM T{}", " UNION SELECT ID FROM T".repeat(n))),
        ("UNION parentheses", |n| format!("{}SELECT ID FROM T{}", "(".repeat(n), ")".repeat(n))),
        ("EXCEPT chain", |n| format!("SELECT ID FROM T{}", " EXCEPT ALL VALUES 9".repeat(n))),
        ("INTERSECT chain", |n| format!("SELECT ID FROM T{}", " INTERSECT SELECT ID FROM T".repeat(n))),
        ("subquery parentheses", |n| format!("SELECT ID FROM T WHERE ID IN {}SELECT ID FROM T{}", "(".repeat(n), ")".repeat(n))),
        // One plan level per table; the pushed-down A0.ID = 0 keeps the product empty.
        ("FROM list", |n| {
            let tables: Vec<String> = (0..=n).map(|i| format!("T A{i}")).collect();
            format!("SELECT A0.ID FROM {} WHERE A0.ID = 0", tables.join(", "))
        }),
        // WHERE and HAVING conjuncts, which rewrites gather into one condition.
        ("WHERE and HAVING chains", |n| {
            let chain = " AND ID = 1".repeat(n);
            format!("SELECT ID FROM T WHERE ID = 1{chain} GROUP BY ID HAVING ID = 1{chain}")
        }),
    ]
}

/// Runs `sql` on a thread with `HALF_DEFAULT_STACK`; a stack overflow aborts the test.
fn rows_on_half_stack(sql: String) -> Result<usize, Code> {
    std::thread::Builder::new()
        .stack_size(HALF_DEFAULT_STACK)
        .spawn(move || rows(&setup().session("APP"), &sql).map(|r| r.len()))
        .expect("the thread starts")
        .join()
        .expect("the statement does not panic")
}

/// Verifies: [DESIGN] `MAX_NESTING_DEPTH` (RULES.md R-CF-6): a statement nested
/// deeper than the limit is rejected with SQLCODE -900, never a stack overflow,
/// and the deepest accepted statement of each shape runs in half the default stack.
///
/// Regression test: 100,000 nested
/// parentheses used to abort the process.
#[test]
fn nesting_beyond_the_limit_is_rejected_not_a_crash() {
    for (shape, sql) in nesting_shapes() {
        assert_eq!(rows_on_half_stack(sql(VERY_DEEP)), Err(Code::Unsupported), "{shape}");
        let first_rejected = (1..=MAX_NESTING_DEPTH)
            .find(|&n| rows_on_half_stack(sql(n)).is_err())
            .unwrap_or_else(|| panic!("{shape}: never rejected within {MAX_NESTING_DEPTH}"));
        assert_eq!(rows_on_half_stack(sql(first_rejected)), Err(Code::Unsupported), "{shape}");
        assert!(rows_on_half_stack(sql(first_rejected - 1)).is_ok(), "{shape}: deepest accepted statement");
    }
}

/// Verifies: [DESIGN] SQLCODE -900 for valid SQL-86 not yet implemented:
/// embedded SQL's `<variable specification>` ([X3.135-1986 §5.6 SR5])
#[test]
fn unsupported_is_reported_as_such() {
    let db = setup();
    assert_eq!(code(&db.session("APP"), "SELECT ID FROM T WHERE ID = :X"), Code::Unsupported);
}

/// Verifies: [X3.135-1986 §5.9 GR4] monadic plus does not change its operand,
/// and monadic minus reverses its sign, on columns and on folded constants.
#[test]
fn monadic_operators_5_9_gr4() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT +AMT, -AMT, -(ID), -(1 + 2) FROM T WHERE ID = 1"), Ok(vec!["10.50,-10.50,-1,-3".into()]));
    assert_eq!(rows(&s, "SELECT -F FROM T WHERE ID = 1"), Ok(vec!["-1.5E0".into()]));
}

/// Verifies: [X3.135-1986 §5.18 GR2] NOT of a true condition is false and of a
/// false one true, also when the condition is constant and folded at planning.
#[test]
fn not_of_a_constant_condition_5_18_gr2() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT (1 = 2) ORDER BY ID"), Ok(vec!["1".into(), "2".into(), "3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT (1 = 1)"), Ok(vec![]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1 OR NOT (2 < 1) AND ID = 2 ORDER BY ID"), Ok(vec!["1".into(), "2".into()]));
}

/// Verifies: [DESIGN] an error's text names the character where it was found.
#[test]
fn errors_name_their_position() {
    let db = setup();
    let s = db.session("APP");
    let error = s.open_cursor("SELECT ID FROM T WHERE ID = !").err().map(|e| e.to_string());
    assert!(error.as_deref().is_some_and(|e| e.contains("at character 28")), "{error:?}");
}
