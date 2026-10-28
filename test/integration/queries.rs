//! Query specifications, set functions, grouping, ORDER BY and UNION
//! [X3.135-1986 §5.8, §5.22-§5.25, §8.3].

use turborel::db::Database;
use turborel::error::Code;
use turborel::functions::AVG_EXTRA_SCALE;
use turborel::value::NULLS_SORT_HIGH;

use crate::common::{code, rows, setup};

/// Verifies: [X3.135-1986 §5.25 SR9] only a plain column reference gives its result column a name.
#[test]
fn only_a_plain_column_names_its_result_column_5_25() {
    let db = setup();
    let s = db.session("APP");
    let c = s.open_cursor("SELECT ID, ID + 1, (ID) FROM T").unwrap();
    let names: Vec<Option<String>> = c.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, vec![Some("ID".into()), None, None]); // [X3.135-1986 §5.25 SR9]
}

/// Verifies: [X3.135-1986 §5.25] "*" selects every column of the table.
#[test]
fn star_selects_every_column_5_25() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT * FROM U ORDER BY 2"), Ok(vec!["1,A  ".into(), "1,B  ".into()]));
}

/// Verifies: [X3.135-1986 §5.25] DISTINCT removes duplicate rows and ALL keeps them.
#[test]
fn distinct_removes_duplicate_rows_5_25() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT DISTINCT ID FROM U"), Ok(vec!["1".into()]));
    assert_eq!(rows(&s, "SELECT ALL ID FROM U").map(|r| r.len()), Ok(2));
}

/// Verifies: [X3.135-1986 §5.11 GR7] for DISTINCT, nulls are duplicates of each other.
#[test]
fn distinct_treats_nulls_as_duplicates_5_11() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.11 GR7] nulls are duplicates of each other for DISTINCT: rows 3 and 4 give one.
    s.run("INSERT INTO T (ID) VALUES (4)").unwrap();
    assert_eq!(rows(&s, "SELECT DISTINCT NAME FROM T WHERE ID > 2"), Ok(vec!["NULL".into()]));
}

/// Verifies: [X3.135-1986 §8.3 F] a query term may be a parenthesized query expression.
#[test]
fn a_query_term_may_be_parenthesized_8_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "((SELECT ID FROM T WHERE ID = 1))"), Ok(vec!["1".into()])); // [X3.135-1986 §8.3] <query term>
}

/// Verifies: [X3.135-1986 §5.6 GR4] USER is the session's authorization identifier.
#[test]
fn user_is_the_authorization_identifier_5_6() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT USER FROM T WHERE ID = 1"), Ok(vec!["APP".into()])); // [X3.135-1986 §5.6 GR4]
}

/// Verifies: [X3.135-1986 §5.8 GR1, GR2, GR4a, GR4b] set functions ignore nulls;
/// COUNT(*) counts rows and COUNT(DISTINCT) counts values.
#[test]
fn set_functions_eliminate_nulls_5_8() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.8 GR1, GR2] nulls are eliminated; [X3.135-1986 §5.8 GR4a, GR4b] COUNT(*) counts rows, COUNT(DISTINCT) values.
    assert_eq!(
        rows(&s, "SELECT COUNT(*), COUNT(DISTINCT NAME), SUM(AMT), MAX(NAME), MIN(AMT) FROM T"),
        Ok(vec!["3,2,7.50,BOB   ,-3.00".into()])
    );
}

/// Verifies: [X3.135-1986 §5.8 SR9b] SUM keeps the scale of its argument; AVG's
/// implementor-defined scale is the argument's plus `AVG_EXTRA_SCALE`.
#[test]
fn sum_keeps_the_scale_and_avg_adds_extra_scale_5_8() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.8 SR9b] SUM keeps the scale. AVG's scale is implementor-defined:
    // the argument's plus AVG_EXTRA_SCALE.
    let avg = |value: f64, scale: u32| format!("{value:.*}", usize::try_from(scale + AVG_EXTRA_SCALE).unwrap());
    assert_eq!(rows(&s, "SELECT AVG(AMT), AVG(ID), AVG(F) FROM T"), Ok(vec![format!("{},{},1.75E0", avg(3.75, 2), avg(2.0, 0))]));
}

/// Verifies: [X3.135-1986 §5.8 GR4c] a set function of an empty argument is null, except COUNT.
#[test]
fn set_functions_of_no_rows_are_null_except_count_5_8() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §5.8 GR4c] empty argument gives null, except COUNT.
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(AMT), AVG(AMT), MAX(ID) FROM T WHERE ID > 99"), Ok(vec!["0,NULL,NULL,NULL".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 F, SR1] a set function
/// takes DISTINCT or ALL, or neither, before any value expression: COUNT of
/// one counts its values not null, which SQL-86 does not allow
/// [X3.135-1986 §5.8 F]; only COUNT takes *.
#[test]
fn set_function_arguments_follow_the_grammar_6_5() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(ID), COUNT(ALL NAME), COUNT(DISTINCT AMT + 1) FROM T"), Ok(vec!["3,2,2".into()]));
    assert_eq!(code(&s, "SELECT SUM(*) FROM T"), Code::Syntax);
    assert_eq!(rows(&s, "SELECT SUM(ALL ID * 2), SUM(DISTINCT ID), SUM(DISTINCT ID + 1) FROM T"), Ok(vec!["12,6,9".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 SR4, SR9a] an argument
/// holds no set function and no subquery, but need not reference a column,
/// which SQL-86 requires [X3.135-1986 §5.8 SR4]; SUM and AVG take no
/// character strings.
#[test]
fn set_function_arguments_follow_the_syntax_rules_6_5() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT SUM(2), COUNT(1) FROM T"), Ok(vec!["6,3".into()]));
    assert_eq!(code(&s, "SELECT AVG(NAME) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT MAX(MIN(ID)) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT MAX((SELECT MAX(ID) FROM U)) FROM T"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §5.25 SR7] a select list with a set function may not
/// also reference an ungrouped column.
#[test]
fn a_set_function_does_not_mix_with_ungrouped_columns_5_25() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT SUM(ID) * AMT FROM T"), Code::SyntaxRule); // [X3.135-1986 §5.25 SR7]
}

/// Verifies: [X3.135-1986 §5.21] a WHERE clause may not contain a set function.
#[test]
fn where_has_no_set_function_5_21() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE COUNT(*) > 1"), Code::SyntaxRule); // [X3.135-1986 §5.21]
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.11, §7.9] a DISTINCT set
/// function may have dyadic operators around it, and a query specification
/// any number of DISTINCTs: restrictions of SQL-86 [X3.135-1986 §5.9 SR1,
/// §5.25 SR5] that SQL-92 keeps for Intermediate SQL alone [§7.9,
/// Leveling Rules].
#[test]
fn distinct_set_functions_combine_7_9() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(DISTINCT ID) * 2 FROM T"), Ok(vec!["6".into()]));
    assert_eq!(rows(&s, "SELECT SUM(ID) * 2 + COUNT(*) FROM T"), Ok(vec!["15".into()]));
    assert_eq!(rows(&s, "SELECT DISTINCT COUNT(DISTINCT ID), COUNT(DISTINCT NAME) FROM T"), Ok(vec!["3,2".into()]));
}

/// `setup()` plus row 4 ('ANN', 1) and row 5 (all null but ID), for the grouping tests.
fn grouping_rows() -> Database {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (4, 'ANN', 1, 0)").unwrap();
    s.run("INSERT INTO T (ID) VALUES (5)").unwrap();
    drop(s);
    db
}

/// Verifies: [X3.135-1986 §5.22 GR2], [X3.135-1986 §5.11 GR7] GROUP BY makes one
/// group per distinct value, with nulls in one group.
#[test]
fn group_by_makes_one_group_per_value_5_22() {
    let db = grouping_rows();
    let s = db.session("APP");
    // [X3.135-1986 §5.22 GR2] one group per distinct value, nulls together [X3.135-1986 §5.11 GR7]; 'ANN' = 'ANN   '.
    let mut got = rows(&s, "SELECT NAME, COUNT(*), SUM(AMT) FROM T GROUP BY NAME").unwrap();
    got.sort();
    assert_eq!(got, vec!["ANN   ,2,11.50", "BOB   ,1,-3.00", "NULL,2,NULL"]);
}

/// Verifies: [X3.135-1986 §5.23] HAVING keeps the groups whose condition is true.
#[test]
fn having_filters_groups_5_23() {
    let db = grouping_rows();
    let s = db.session("APP");
    // [X3.135-1986 §5.23] HAVING filters groups; columns outside set functions must be grouping columns.
    assert_eq!(rows(&s, "SELECT NAME FROM T GROUP BY NAME HAVING COUNT(*) > 1 AND NAME IS NOT NULL"), Ok(vec!["ANN   ".into()]));
}

/// Verifies: [X3.135-1986 §5.23] a column in HAVING outside a set function must be a grouping column.
#[test]
fn having_references_only_grouping_columns_5_23() {
    let db = grouping_rows();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT NAME FROM T GROUP BY NAME HAVING ID > 1"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §5.25 SR7] a grouped select list references only grouping columns.
#[test]
fn a_grouped_select_list_has_only_grouping_columns_5_25() {
    let db = grouping_rows();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T GROUP BY NAME"), Code::SyntaxRule); // [X3.135-1986 §5.25 SR7]
    assert_eq!(code(&s, "SELECT * FROM T GROUP BY NAME"), Code::SyntaxRule); // "*" is every column
}

/// Verifies: [X3.135-1986 §5.22 GR2] with two grouping columns, rows group only
/// when both agree, nulls together.
#[test]
fn rows_group_only_when_every_grouping_column_agrees_5_22() {
    let db = grouping_rows();
    let s = db.session("APP");
    // Two grouping columns: rows group only when both agree (nulls together).
    assert_eq!(rows(&s, "SELECT NAME, AMT, COUNT(*) FROM T WHERE NAME IS NULL GROUP BY NAME, AMT"), Ok(vec!["NULL,NULL,2".into()]));
    let mut ann = rows(&s, "SELECT NAME, AMT, COUNT(*) FROM T WHERE NAME = 'ANN' GROUP BY NAME, AMT").unwrap();
    ann.sort();
    assert_eq!(ann, vec!["ANN   ,1.00,1", "ANN   ,10.50,1"]);
}

/// Verifies: [X3.135-1986 §5.22 SR2] a grouping column must be a column of the table.
#[test]
fn a_grouping_column_must_exist_5_22() {
    let db = grouping_rows();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT COUNT(*) FROM T GROUP BY NOPE"), Code::SyntaxRule); // [X3.135-1986 §5.22 SR2]
}

/// Verifies: [X3.135-1986 §5.23 GR1] HAVING without GROUP BY treats the whole table as one group.
#[test]
fn having_without_group_by_makes_one_group_5_23() {
    let db = grouping_rows();
    let s = db.session("APP");
    // HAVING without GROUP BY: all of R is one group [X3.135-1986 §5.23 GR1].
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T HAVING COUNT(*) > 3"), Ok(vec!["5".into()]));
}

/// Verifies: [X3.135-1986 §5.25 GR3] zero groups with a select list of only set
/// functions give one row, but no row when the select list has a column.
#[test]
fn zero_groups_give_one_row_of_set_functions_5_25() {
    let db = grouping_rows();
    let s = db.session("APP");
    // [X3.135-1986 §5.25 GR3], read literally: a grouped table with zero groups and a select
    // list of only set functions gives one row, here because HAVING removed the group.
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T HAVING COUNT(*) > 99"), Ok(vec!["0".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(ID) FROM T WHERE ID > 99 GROUP BY NAME"), Ok(vec!["0,NULL".into()]));
    // ...but not when the select list has a column.
    assert_eq!(rows(&s, "SELECT NAME, COUNT(*) FROM T WHERE ID > 99 GROUP BY NAME"), Ok(vec![]));
}

/// Verifies: [X3.135-1986 §8.3] ORDER BY sorts by position, name or qualified
/// name, ascending or descending, on one or more keys.
#[test]
fn order_by_sorts_by_its_keys_8_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY ID DESC"), Ok(vec!["3".into(), "2".into(), "1".into()]));
    // By position, by name, by qualified name, several keys.
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY 2"), Ok(vec!["2,-3.00".into(), "1,10.50".into(), "3,NULL".into()]));
    assert_eq!(rows(&s, "SELECT X.ID FROM T X ORDER BY X.ID DESC").map(|r| r[0].clone()), Ok("3".into()));
    assert_eq!(rows(&s, "SELECT ID, CODE FROM U ORDER BY ID, CODE DESC"), Ok(vec!["1,B  ".into(), "1,A  ".into()]));
}

/// Verifies: [X3.135-1986 §8.3 GR3b vii] nulls sort on the side set by `NULLS_SORT_HIGH`.
#[test]
fn nulls_sort_on_one_side_8_3() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.3 GR3b vii] where nulls sort is implementor-defined: NULLS_SORT_HIGH.
    let ascending: Vec<String> = if NULLS_SORT_HIGH {
        vec!["ANN   ".into(), "BOB   ".into(), "NULL".into()]
    } else {
        vec!["NULL".into(), "ANN   ".into(), "BOB   ".into()]
    };
    assert_eq!(rows(&s, "SELECT NAME FROM T ORDER BY NAME"), Ok(ascending.clone()));
    assert_eq!(rows(&s, "SELECT NAME FROM T ORDER BY NAME DESC").map(|r| r[0].clone()), Ok(ascending[2].clone()));
}

/// Verifies: [X3.135-1986 §8.3 SR6] a sort key must be a column of the result.
#[test]
fn a_sort_key_must_be_a_result_column_8_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM U ORDER BY CODE"), Code::SyntaxRule); // must be a column of the result
    assert_eq!(code(&s, "SELECT ID FROM T ORDER BY NAME"), Code::SyntaxRule); // not a column of the result
}

/// Verifies: [X3.135-1986 §8.3 SR6] a sort position is greater than 0 and not
/// greater than the number of result columns.
#[test]
fn a_sort_position_must_be_within_the_select_list_8_3() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.3 SR6]
    assert_eq!(code(&s, "SELECT ID FROM T ORDER BY 2"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T ORDER BY 0"), Code::SyntaxRule);
}

/// Verifies: [X3.135-1986 §8.3 SR6] an unnamed result column is sorted by position only.
#[test]
fn an_unnamed_column_is_sorted_by_position_8_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID + 1 FROM T ORDER BY ID"), Code::SyntaxRule); // unnamed column
    assert_eq!(rows(&s, "SELECT ID + 1 FROM T ORDER BY 1 DESC").map(|r| r[0].clone()), Ok("4".into()));
}

/// Verifies: [X3.135-1986 §8.3 SR6] a sort key name must identify one result column.
#[test]
fn a_sort_key_must_be_unambiguous_8_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT X.ID, Y.ID FROM T X, T Y ORDER BY ID"), Code::SyntaxRule); // ambiguous
    assert_eq!(rows(&s, "SELECT X.ID, Y.ID FROM T X, T Y WHERE X.ID = 1 ORDER BY Y.ID DESC").map(|r| r[0].clone()), Ok("1,3".into()));
}

/// Verifies: [X3.135-1986 §8.3 GR2b] UNION removes duplicate rows and UNION ALL keeps them.
#[test]
fn union_removes_duplicates_and_union_all_keeps_them_8_3() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.3 GR2b] UNION removes duplicates, UNION ALL keeps them.
    assert_eq!(rows(&s, "SELECT ID FROM T UNION SELECT ID FROM T").map(|r| r.len()), Ok(3));
    assert_eq!(rows(&s, "SELECT ID FROM T UNION ALL SELECT ID FROM T").map(|r| r.len()), Ok(6));
    assert_eq!(
        rows(&s, "SELECT ID FROM T WHERE ID = 1 UNION (SELECT ID FROM U UNION ALL SELECT ID FROM T WHERE ID > 2) ORDER BY 1 DESC"),
        Ok(vec!["3".into(), "1".into()])
    );
}
