//! Unit tests of the Format of the `<merge statement>` [ISO/IEC
//! 9075-2:2023 §14.12]: each kind of WHEN clause, the correlation name,
//! the source, and the refusal of text that breaks the Format.

use super::*;
use crate::ast::{Overriding, RowElement, SetClause, TableReference, UpdateSource};
use crate::error::Code;

/// The `<merge statement>` `text` is, parsed as a whole statement.
fn merge(text: &str) -> MergeStatement {
    match Parser::new(text).and_then(|mut p| p.sql_statement()) {
        Ok(SqlStatement::Merge(m)) => *m,
        other => panic!("{text}: {other:?}"),
    }
}

/// The SQLCODE `text`, parsed as a whole statement, fails with.
fn refused(text: &str) -> Option<Code> {
    Parser::new(text).and_then(|mut p| p.sql_statement()).err().map(|e| e.code)
}

/// The insert specification of the single WHEN NOT MATCHED clause of
/// MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN `specification`.
fn insert(specification: &str) -> MergeInsert {
    let mut m = merge(&format!("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN {specification}"));
    match m.clauses.pop() {
        Some(MergeWhen::NotMatched { insert, .. }) => insert,
        other => panic!("{specification}: {other:?}"),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] WHEN MATCHED THEN UPDATE SET
/// reads its `<set clause list>` in order, of NULL, DEFAULT and value
/// expressions, and WHEN begins the clause.
#[test]
fn matched_update_reads_its_set_clauses_14_12() {
    let m = merge("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN UPDATE SET A = S.A, B = NULL, C = DEFAULT");
    let [MergeWhen::Matched { condition: None, action: MatchedAction::Update(set), at }] = m.clauses.as_slice() else {
        panic!("{:?}", m.clauses)
    };
    assert_eq!(*at, 34);
    let columns: Vec<&str> = set.iter().map(|s| s.column.as_str()).collect();
    assert_eq!(columns, ["A", "B", "C"]);
    assert!(matches!(
        set.as_slice(),
        [
            SetClause { value: UpdateSource::Value(_), .. },
            SetClause { value: UpdateSource::Null, .. },
            SetClause { value: UpdateSource::Default, .. }
        ]
    ));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] WHEN MATCHED THEN DELETE
/// \[Feature F314\], and the target, source and ON condition read.
#[test]
fn matched_delete_14_12() {
    let m = merge("MERGE INTO A.T USING S ON TRUE WHEN MATCHED THEN DELETE");
    assert_eq!((m.target.auth.as_deref(), m.target.ident.as_str()), (Some("A"), "T"));
    assert!(matches!(&m.source, TableReference::Table(t) if t.table.ident == "S" && t.correlation.is_none()));
    assert!(matches!(m.clauses.as_slice(), [MergeWhen::Matched { condition: None, action: MatchedAction::Delete, .. }]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] WHEN NOT MATCHED THEN INSERT
/// with no column list and none of OVERRIDING, its values in parentheses
/// in order, NULL and DEFAULT among them, INSERT where it begins.
#[test]
fn not_matched_insert_without_columns_14_12() {
    let i = insert("INSERT VALUES (S.K, NULL, DEFAULT, S.A + 1)");
    assert!(i.columns.is_none() && i.overriding.is_none());
    assert_eq!(i.at, 56);
    assert!(matches!(
        i.values.elements.as_slice(),
        [RowElement::Value(..), RowElement::Null(_), RowElement::Default(_), RowElement::Value(..)]
    ));
    let one = insert("INSERT VALUES (1)");
    assert_eq!(one.values.elements.len(), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] an `<insert column list>`, each
/// column where it is written, and an `<override clause>` of USER VALUE
/// and of SYSTEM VALUE, after the column list or alone.
#[test]
fn not_matched_insert_with_columns_and_override_14_12() {
    let i = insert("INSERT (K, A) OVERRIDING USER VALUE VALUES (S.K, S.A)");
    let names: Vec<(&str, usize)> = i.columns.iter().flatten().map(|c| (c.name.as_str(), c.at)).collect();
    assert_eq!(names, [("K", 64), ("A", 67)]);
    assert!(matches!(i.overriding, Some(Overriding::User)));
    let system = insert("INSERT OVERRIDING SYSTEM VALUE VALUES (S.K)");
    assert!(system.columns.is_none() && matches!(system.overriding, Some(Overriding::System)));
    let listed = insert("INSERT (K) VALUES (S.K)");
    assert!(listed.columns.is_some_and(|c| c.len() == 1) && listed.overriding.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] AND `<search condition>` on a
/// matched and a not matched clause, and several clauses of both kinds in
/// the order written \[Feature F313\].
#[test]
fn several_clauses_with_conditions_in_order_14_12() {
    let m = merge(
        "MERGE INTO T USING S ON T.K = S.K \
         WHEN NOT MATCHED AND S.A > 0 THEN INSERT VALUES (S.K) \
         WHEN MATCHED AND S.A IS NULL THEN DELETE \
         WHEN MATCHED THEN UPDATE SET A = S.A \
         WHEN NOT MATCHED THEN INSERT VALUES (S.K)",
    );
    assert!(matches!(
        m.clauses.as_slice(),
        [
            MergeWhen::NotMatched { condition: Some(_), .. },
            MergeWhen::Matched { condition: Some(_), action: MatchedAction::Delete, .. },
            MergeWhen::Matched { condition: None, action: MatchedAction::Update(_), .. },
            MergeWhen::NotMatched { condition: None, .. }
        ]
    ));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] the `<merge correlation name>`
/// after AS, without AS, and not written, each where its name begins.
#[test]
fn correlation_name_with_and_without_as_14_12() {
    let as_written = merge("MERGE INTO T AS X USING S ON TRUE WHEN MATCHED THEN DELETE");
    assert_eq!(as_written.correlation, Some(("X".to_owned(), 16)));
    let bare = merge("MERGE INTO T X USING S ON TRUE WHEN MATCHED THEN DELETE");
    assert_eq!(bare.correlation, Some(("X".to_owned(), 13)));
    let none = merge("MERGE INTO T USING S ON TRUE WHEN MATCHED THEN DELETE");
    assert!(none.correlation.is_none());
    assert_eq!(refused("MERGE INTO T AS USING S ON TRUE WHEN MATCHED THEN DELETE"), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] the source a `<table
/// reference>` of any kind: a named derived table, a parenthesized joined
/// table, and a joined table, which the binder refuses \[SR11\].
#[test]
fn source_is_any_table_reference_14_12() {
    let derived = merge("MERGE INTO T USING (SELECT K FROM U) AS S (K) ON T.K = S.K WHEN MATCHED THEN DELETE");
    assert!(matches!(&derived.source, TableReference::Derived(d) if d.correlation == "S"));
    let parenthesized = merge("MERGE INTO T USING (U JOIN V ON U.K = V.K) ON T.K = U.K WHEN MATCHED THEN DELETE");
    assert!(matches!(parenthesized.source, TableReference::Joined(_)));
    let joined = merge("MERGE INTO T USING U JOIN V ON U.K = V.K ON T.K = U.K WHEN MATCHED THEN DELETE");
    assert!(matches!(joined.source, TableReference::Joined(_)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12] text breaking the Format is a
/// syntax error, SQLCODE -102: no WHEN clause, no THEN, no MATCHED,
/// UPDATE without SET, INSERT without VALUES, no ON, no USING, an empty or
/// unparenthesized VALUES list, the wrong action for each kind of clause,
/// and text after the last clause.
#[test]
fn format_refusals_14_12() {
    for text in [
        "MERGE INTO T USING S ON TRUE",
        "MERGE INTO T USING S ON TRUE WHEN MATCHED DELETE",
        "MERGE INTO T USING S ON TRUE WHEN DELETE",
        "MERGE INTO T USING S ON TRUE WHEN MATCHED THEN UPDATE A = 1",
        "MERGE INTO T USING S ON TRUE WHEN NOT MATCHED THEN INSERT (A)",
        "MERGE INTO T USING S WHEN MATCHED THEN DELETE",
        "MERGE INTO T S ON TRUE WHEN MATCHED THEN DELETE",
        "MERGE INTO T USING S ON TRUE WHEN NOT MATCHED THEN INSERT VALUES ()",
        "MERGE INTO T USING S ON TRUE WHEN NOT MATCHED THEN INSERT VALUES 1",
        "MERGE INTO T USING S ON TRUE WHEN NOT MATCHED THEN INSERT () VALUES (1)",
        "MERGE INTO T USING S ON TRUE WHEN NOT MATCHED THEN DELETE",
        "MERGE INTO T USING S ON TRUE WHEN MATCHED THEN INSERT VALUES (1)",
        "MERGE INTO T USING S ON TRUE WHEN MATCHED THEN DELETE DELETE",
    ] {
        assert_eq!(refused(text), Some(Code::Syntax), "{text}");
    }
}
