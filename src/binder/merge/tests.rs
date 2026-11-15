//! Unit tests of a MERGE's binding: its refusals, the layout of its rows
//! and the flags of its WHEN NOT MATCHED clauses (RULES.md R-TEST-1).

use std::sync::Arc;

use super::*;
use crate::ast::{MergeWhen, SqlStatement};
use crate::catalog::QualifiedName;
use crate::db::{Database, State};
use crate::error::{Code, SqlError};
use crate::params::Params;
use crate::parser::Parser;
use crate::value::Value;

/// A schema of S: table T, to merge into, tables U and X, sources, an
/// updatable view of T's rows of V over 0, a read-only view, and R's
/// privileges, UPDATE of T's V alone and SELECT on U.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER PRIMARY KEY, V INTEGER, X INTEGER)
           CREATE TABLE U (K INTEGER, V INTEGER, W INTEGER)
           CREATE TABLE Y (K INTEGER)
           CREATE VIEW P AS SELECT K, V FROM T WHERE V > 0
           CREATE VIEW D AS SELECT DISTINCT K, V FROM T
           GRANT UPDATE (V) ON T TO R
           GRANT SELECT ON U TO R",
    )
    .unwrap();
    db
}

/// The snapshot of `database()`, the MERGE `sql`, and the object columns
/// of each of its WHEN NOT MATCHED clauses: the columns of S.T its insert
/// column list names, or every one.
fn prepared(sql: &str) -> (Arc<State>, Box<MergeStatement>, Vec<Vec<ObjectColumn>>) {
    let st = database().snapshot().unwrap();
    let Ok(SqlStatement::Merge(stmt)) = Parser::new(sql).unwrap().sql_statement() else { panic!("not a MERGE: {sql}") };
    let base = st.catalog.table(&QualifiedName { auth: "S".into(), ident: "T".into() }).unwrap().clone();
    let object = |name: &str| ObjectColumn { desc: base.columns[base.column(name).unwrap()].clone(), default: Value::Null, draws: None };
    let inserts: Vec<Vec<ObjectColumn>> = stmt
        .clauses
        .iter()
        .filter_map(|clause| match clause {
            MergeWhen::NotMatched { insert, .. } => Some(match &insert.columns {
                Some(names) => names.iter().map(|n| object(&n.name)).collect(),
                None => base.columns.iter().map(|col| object(&col.name)).collect(),
            }),
            MergeWhen::Matched { .. } => None,
        })
        .collect();
    (st, stmt, inserts)
}

/// `sql`, a MERGE, bound as `user` binds it (`prepared`).
fn bound_as(user: &str, sql: &str) -> Result<BoundMerge> {
    let (st, stmt, inserts) = prepared(sql);
    let params = Params::new();
    bind_merge(Context::new(&st, user, &params), &stmt, &inserts)
}

/// `sql` bound as S, the owner.
fn bound(sql: &str) -> Result<BoundMerge> {
    bound_as("S", sql)
}

/// The error binding `sql` as `user` gives.
fn refused_as(user: &str, sql: &str) -> SqlError {
    bound_as(user, sql).err().unwrap_or_else(|| panic!("not refused: {sql}"))
}

/// Whether `e` cites `rule`.
fn cites(e: &SqlError, rule: &str) -> bool {
    e.rule.as_deref().is_some_and(|r| r.contains(rule))
}

/// Whether `e` is a Syntax Rule violation citing `rule`.
fn breaks(e: &SqlError, rule: &str) -> bool {
    e.code == Code::SyntaxRule && cites(e, rule)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a ii 1 to 3] the rows are
/// T's base columns, the source's, a flag for each WHEN NOT MATCHED clause
/// of a search condition alone, and the row id; each clause bound over
/// them, SET and insert columns as base table positions.
#[test]
fn rows_are_target_source_flags_and_row_id_14_12() {
    let merge = bound(
        "MERGE INTO T USING U ON T.K = U.K
           WHEN MATCHED AND U.V > 0 THEN UPDATE SET V = U.V
           WHEN MATCHED THEN DELETE
           WHEN NOT MATCHED AND U.V > 1 THEN INSERT VALUES (U.K, U.V, W)
           WHEN NOT MATCHED THEN INSERT (V, K) VALUES (W, U.K)",
    )
    .unwrap();
    assert_eq!(merge.source_width, 3);
    assert_eq!(merge.rows.width(), 3 + 3 + 1 + 1, "T's columns, U's, one flag, the row id");
    assert_eq!(merge.table.ident, "T");
    let [matched, deleted, flagged, unflagged] = merge.clauses.as_slice() else { panic!("four clauses") };
    let BoundWhen::Matched { condition: Some(_), action: BoundAction::Update(set) } = matched else { panic!("an update: {matched:?}") };
    assert_eq!(set.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1], "V");
    assert!(matches!(deleted, BoundWhen::Matched { condition: None, action: BoundAction::Delete }));
    let BoundWhen::NotMatched { flag: Some(6), object, values } = flagged else { panic!("flagged: {flagged:?}") };
    assert_eq!((object.as_slice(), values.len()), ([0, 1, 2].as_slice(), 3));
    let BoundWhen::NotMatched { flag: None, object: columns, .. } = unflagged else { panic!("unflagged: {unflagged:?}") };
    assert_eq!(*columns, [1, 0]);
    assert!(matches!(merge.on, Cond::Compare(..)), "a base table has no view's condition");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a ii 1 to 3] each WHEN NOT
/// MATCHED clause of a search condition takes the next flag, in order,
/// after the source's columns; one of none takes none.
#[test]
fn flags_follow_in_order_14_12() {
    let merge = bound(
        "MERGE INTO T AS M USING U ON M.K = U.K
           WHEN NOT MATCHED AND U.V = 1 THEN INSERT (K) VALUES (U.K)
           WHEN NOT MATCHED THEN INSERT (K) VALUES (U.K)
           WHEN NOT MATCHED AND M.X IS NULL THEN INSERT (K) VALUES (U.K)",
    )
    .unwrap();
    let flags: Vec<Option<usize>> = merge
        .clauses
        .iter()
        .map(|c| match c {
            BoundWhen::NotMatched { flag, .. } => *flag,
            BoundWhen::Matched { .. } => panic!("no WHEN MATCHED clause"),
        })
        .collect();
    assert_eq!(flags, [Some(6), None, Some(7)]);
    assert_eq!(merge.rows.width(), 3 + 3 + 2 + 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR7, SR8a] a view is changed as
/// its base table, its condition in what a changed row is matched again
/// by; the merge correlation name is the target's exposed name.
#[test]
fn a_view_is_changed_as_its_base_table_14_12() {
    let merge = bound("MERGE INTO P AS Q USING U ON Q.K = U.K WHEN MATCHED THEN UPDATE SET V = U.V").unwrap();
    assert_eq!((merge.named.ident.as_str(), merge.table.ident.as_str()), ("P", "T"));
    assert!(matches!(merge.on, Cond::And(..)), "SC1 and the view's condition");
    assert_eq!(merge.rows.width(), 3 + 3 + 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR11] the source is no joined
/// table.
#[test]
fn a_joined_source_is_refused_14_12() {
    let e = refused_as("S", "MERGE INTO T USING U JOIN Y ON U.K = Y.K ON T.K = U.K WHEN MATCHED THEN DELETE");
    assert!(breaks(&e, "§14.12 SR11"), "{e:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR12] the source exposes no name
/// the target is exposed by: of two table names, two correlation names,
/// or a correlation name and the other's table name, either way.
#[test]
fn the_same_exposed_name_is_refused_14_12() {
    for sql in [
        "MERGE INTO T USING T ON T.K = 1 WHEN MATCHED THEN DELETE",
        "MERGE INTO T AS A USING U AS A ON A.K = 1 WHEN MATCHED THEN DELETE",
        "MERGE INTO T AS U USING U ON U.K = 1 WHEN MATCHED THEN DELETE",
        "MERGE INTO T USING U AS T ON T.K = 1 WHEN MATCHED THEN DELETE",
    ] {
        let e = refused_as("S", sql);
        assert!(breaks(&e, "§14.12 SR12"), "{sql}: {e:?}");
    }
    assert!(bound("MERGE INTO T AS A USING U AS B ON A.K = B.K WHEN MATCHED THEN DELETE").is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR18d] as many insert values as
/// insert columns, fewer or more refused.
#[test]
fn insert_values_count_their_columns_14_12() {
    for values in ["(U.K)", "(U.K, U.V, U.W, 1)"] {
        let sql = format!("MERGE INTO T USING U ON T.K = U.K WHEN NOT MATCHED THEN INSERT VALUES {values}");
        let e = refused_as("S", &sql);
        assert!(breaks(&e, "§14.12 SR18d"), "{values}: {e:?}");
    }
    assert!(bound("MERGE INTO T USING U ON T.K = U.K WHEN NOT MATCHED THEN INSERT (K) VALUES (U.K)").is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR9; §7.6 SR20b] the target is
/// not in scope in a merge insert value list, qualified or not, though it
/// is in a SET clause and a WHEN condition; the source is in both.
#[test]
fn the_target_is_not_in_scope_of_insert_values_14_12() {
    for value in ["T.K", "X"] {
        let sql = format!("MERGE INTO T USING U ON T.K = U.K WHEN NOT MATCHED THEN INSERT (K) VALUES ({value})");
        assert_eq!(refused_as("S", &sql).code, Code::SyntaxRule, "{value}");
    }
    let sql = "MERGE INTO T USING U ON T.K = U.K WHEN MATCHED AND X > W THEN UPDATE SET V = X + W";
    assert!(bound(sql).is_ok());
}

/// Verifies: [X3.135-1986 §5.7 SR3a; ISO/IEC 9075-2:2023 §14.12 SR16] a
/// column neither side has, in the ON condition or as a SET clause's
/// object, is refused; and a set function in a search condition [X3.135-1986
/// §5.8 SR1].
#[test]
fn undefined_columns_and_set_functions_are_refused_14_12() {
    let undefined = refused_as("S", "MERGE INTO T USING U ON T.K = U.NOPE WHEN MATCHED THEN DELETE");
    assert_eq!(undefined.code, Code::SyntaxRule);
    let object = refused_as("S", "MERGE INTO T USING U ON T.K = U.K WHEN MATCHED THEN UPDATE SET NOPE = 1");
    assert!(breaks(&object, "§14.12 SR16"), "{object:?}");
    let twice = refused_as("S", "MERGE INTO T USING U ON T.K = U.K WHEN MATCHED THEN UPDATE SET V = 1, V = 2");
    assert_eq!(twice.code, Code::SyntaxRule);
    let set_function = refused_as("S", "MERGE INTO T USING U ON T.K = MAX(U.K) WHEN MATCHED THEN DELETE");
    assert!(breaks(&set_function, "§5.8 SR1"), "{set_function:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 SR2, SR3, SR4] a read-only target
/// is refused, citing the rule of each kind of WHEN clause written.
#[test]
fn a_read_only_target_is_refused_14_12() {
    let changed = refused_as("S", "MERGE INTO D USING U ON D.K = U.K WHEN MATCHED THEN UPDATE SET V = 1 WHEN MATCHED THEN DELETE");
    assert!(breaks(&changed, "§14.12 SR3, SR4"), "{changed:?}");
    let inserted = refused_as("S", "MERGE INTO D USING U ON D.K = U.K WHEN NOT MATCHED THEN INSERT VALUES (U.K, U.V)");
    assert!(breaks(&inserted, "§14.12 SR2;"), "{inserted:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 AR1b i, AR1b ii; §7.6 AR1b ii 1
/// B] UPDATE on each update object column, DELETE if a clause deletes,
/// and SELECT on the source, each refused if not held, SQLCODE -104.
#[test]
fn privileges_are_required_14_12() {
    let ok = "MERGE INTO S.T USING S.U ON S.T.K = S.U.K WHEN MATCHED THEN UPDATE SET V = S.U.V";
    assert!(bound_as("R", ok).is_ok());
    let cases = [
        ("MERGE INTO S.T USING S.U ON S.T.K = S.U.K WHEN MATCHED THEN UPDATE SET K = 1", "AR1b i"),
        ("MERGE INTO S.T USING S.U ON S.T.K = S.U.K WHEN MATCHED THEN DELETE", "AR1b ii"),
        ("MERGE INTO S.T USING S.Y ON S.T.K = S.Y.K WHEN MATCHED THEN UPDATE SET V = 1", "§7.6 AR1b ii 1 B"),
    ];
    for (sql, rule) in cases {
        let e = refused_as("R", sql);
        assert!(e.code == Code::Privilege && cites(&e, rule), "{sql}: {e:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5] the rows are the source LEFT
/// JOIN T's located rows on SC1, its columns renumbered to the join's
/// source, flags, target and row id, then projected to the target, source,
/// flags and row id.
#[test]
fn rows_are_the_source_left_join_the_located_target_14_12() {
    let merge = bound("MERGE INTO T USING U ON T.K = U.K WHEN MATCHED THEN DELETE").unwrap();
    let LogicalPlan::Project { exprs, input } = &merge.rows else { panic!("a projection: {:?}", merge.rows) };
    let columns: Vec<usize> = exprs.iter().map(|e| if let Expr::Column(c) = e { *c } else { panic!("a column: {e:?}") }).collect();
    assert_eq!(columns, [3, 4, 5, 0, 1, 2, 6]);
    let LogicalPlan::OuterJoin { kind, cond, right, .. } = &**input else { panic!("an outer join: {input:?}") };
    assert_eq!(*kind, crate::logical::OuterJoin::Left);
    assert!(matches!(cond, Cond::Compare(Expr::Column(3), _, Expr::Column(0))), "{cond:?}");
    let LogicalPlan::Scan { names, located, .. } = &**right else { panic!("a scan: {right:?}") };
    assert!(*located);
    assert_eq!(names.last().map(String::as_str), Some(crate::storage::ROW_ID));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a ii 1 to 3] a flag is NOT
/// EXISTS of T's rows for which SC1 OR NOT SCj is true, the source's
/// columns outer references, its outer expressions each source column.
#[test]
fn a_flag_is_not_exists_of_the_target_rows_14_12() {
    let merge = bound("MERGE INTO T USING U ON T.K = U.K WHEN NOT MATCHED AND U.V > 0 THEN INSERT (K) VALUES (U.K)").unwrap();
    let LogicalPlan::Project { input, .. } = &merge.rows else { panic!("a projection") };
    let LogicalPlan::OuterJoin { left, .. } = &**input else { panic!("an outer join") };
    let LogicalPlan::Project { exprs, .. } = &**left else { panic!("the source projected: {left:?}") };
    let Some(Expr::Truth(flag)) = exprs.last() else { panic!("a flag: {exprs:?}") };
    let Cond::Not(exists) = &**flag else { panic!("NOT: {flag:?}") };
    let Cond::Exists(subquery) = &**exists else { panic!("EXISTS: {exists:?}") };
    assert_eq!(subquery.outer.len(), 3, "each source column");
    let LogicalPlan::Filter { cond: Cond::Or(sc1, not_scj), .. } = &subquery.plan else { panic!("a filter: {:?}", subquery.plan) };
    assert!(matches!(&**sc1, Cond::Compare(Expr::Column(0), _, Expr::Outer(0, _))), "{sc1:?}");
    let Cond::Not(scj) = &**not_scj else { panic!("NOT SCj: {not_scj:?}") };
    assert!(matches!(&**scj, Cond::Compare(Expr::Outer(1, _), ..)), "{scj:?}");
}
