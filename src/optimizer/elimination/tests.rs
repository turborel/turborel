//! Unit tests: the keys a join may be removed on, and the joins removed
//! and kept (RULES.md R-TEST-1, R-TEST-8). The rows of the queries are
//! compared with the rule on and off in `test/integration/rewrites`.

use super::*;
use crate::catalog::{ColumnDesc, ConstraintAttributes, ConstraintDesc, MemoryCatalog, QualifiedName};
use crate::db::State;
use crate::value::{ArithOp, DataType, Spelled};

/// Table `ident` of schema S.
fn name(ident: &str) -> QualifiedName {
    QualifiedName { auth: "S".to_owned(), ident: ident.to_owned() }
}

/// An INTEGER column `called`, of collation `collation` if any.
fn column(called: &str, collation: Option<&str>) -> ColumnDesc {
    ColumnDesc {
        name: called.into(),
        data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer },
        not_null: false,
        default: None,
        domain: None,
        collation: collation.map(name),
        generated: None,
    }
}

/// A constraint of `kind`, deferrable if so.
fn constraint(kind: ConstraintKind, deferrable: bool) -> ConstraintDesc {
    ConstraintDesc { name: name("K"), kind, attributes: ConstraintAttributes { deferrable, initially_deferred: false }, domain: None }
}

fn unique(columns: Vec<usize>, primary: bool) -> ConstraintKind {
    ConstraintKind::Unique { columns, primary, nulls_distinct: true, period: None }
}

/// Table `ident` of columns K, V and W, W collated, of `constraints`.
fn table(ident: &str, constraints: Vec<ConstraintDesc>) -> TableDesc {
    TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name(ident),
        columns: vec![column("K", None), column("V", None), column("W", Some("C"))],
        constraints,
        engine: "row".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.2, §4.25.3.2]; [DESIGN] RULES.md
/// D375: a key is one a join may be removed on when its constraint is
/// not deferrable and each of its columns is NOT NULL by a constraint not
/// deferrable, or of the PRIMARY KEY, and of no collation of its own.
///
/// Decision `!deferrable && columns covered && never_null && uncollated`
/// in `unique_key_covered`. MC/DC: (T T T T) covered; each false alone
/// not.
#[test]
fn keys_must_be_unique_not_null_and_not_deferred() {
    let not_null = |at: usize, deferrable: bool| constraint(ConstraintKind::NotNull(at), deferrable);
    let keyed = table("T", vec![constraint(unique(vec![0], false), false), not_null(0, false)]);
    assert!(unique_key_covered(&keyed, &[0]), "T T T T");
    let deferred = table("T", vec![constraint(unique(vec![0], false), true), not_null(0, false)]);
    assert!(!unique_key_covered(&deferred, &[0]), "F T T T");
    assert!(!unique_key_covered(&keyed, &[1]), "T F T T");
    let nullable = table("T", vec![constraint(unique(vec![0], false), false), not_null(0, true)]);
    assert!(!unique_key_covered(&nullable, &[0]), "T T F T");
    let collated = table("T", vec![constraint(unique(vec![2], false), false), not_null(2, false)]);
    assert!(!unique_key_covered(&collated, &[2]), "T T T F");
    let primary = table("T", vec![constraint(unique(vec![0, 1], true), false)]);
    assert!(unique_key_covered(&primary, &[1, 0, 2]) && !unique_key_covered(&primary, &[0]));
    assert!(!unique_within(&deferred, &[0]) && unique_within(&nullable, &[0, 1]));
}

/// Verifies: [DESIGN] RULES.md D375, D15: a condition is quiet unless it
/// can raise of its columns' types or draws a value of a sequence
/// generator.
#[test]
fn conditions_that_can_raise_are_not_quiet() {
    let integer = |_: usize| Some(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer });
    let sum = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Add, Box::new(Expr::Column(1)));
    let quotient = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Div, Box::new(Expr::Column(1)));
    assert!(quiet(&Cond::Compare(sum, CompOp::Eq, Expr::Column(2)), &integer));
    assert!(!quiet(&Cond::Compare(quotient, CompOp::Eq, Expr::Column(2)), &integer));
}

/// The rule applied to the projection `exprs` of `input`, in a database
/// of tables P, keyed by K, and Q, of none.
fn applied(exprs: Vec<Expr>, input: LogicalPlan) -> Option<LogicalPlan> {
    let keyed = table("P", vec![constraint(unique(vec![0], true), false)]);
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("S", "S", vec![keyed, table("Q", Vec::new())]);
    let st = State::with_builtins(Box::new(catalog));
    EliminateJoins
        .apply(&LogicalPlan::Project { exprs, input: Box::new(input) }, &RuleContext { db: &st, seen: &crate::physical::Seen::default() })
}

fn scan(ident: &str) -> LogicalPlan {
    LogicalPlan::Scan { table: name(ident), names: vec!["K".into(), "V".into(), "W".into()], needed: Vec::new(), located: false }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; [DESIGN] RULES.md D375:
/// a LEFT join to P on its key, P's columns unread, is its left operand;
/// a RIGHT join mirrors it; a FULL join, a join on a column not a key, to
/// a table of no key, and one whose P column is read above, are kept.
///
/// Decision `unread && keyed` in `outer_eliminated`. MC/DC: (T T)
/// removed; (F T) kept; (T F) kept.
#[test]
fn outer_joins_on_keys_are_removed() {
    let on = |at: usize| Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(at));
    let join = |kind: OuterJoin, right: &str, at: usize| LogicalPlan::OuterJoin {
        kind,
        cond: on(at),
        left: Box::new(scan("Q")),
        right: Box::new(scan(right)),
    };
    let left_columns = vec![Expr::Column(1), Expr::Column(0)];
    let Some(LogicalPlan::Project { exprs, input }) = applied(left_columns.clone(), join(OuterJoin::Left, "P", 3)) else { panic!("T T") };
    assert!(matches!(*input, LogicalPlan::Scan { .. }) && format!("{exprs:?}") == format!("{left_columns:?}"));
    assert!(applied(vec![Expr::Column(4)], join(OuterJoin::Left, "P", 3)).is_none(), "F T");
    assert!(applied(left_columns.clone(), join(OuterJoin::Left, "P", 4)).is_none(), "T F");
    assert!(applied(left_columns.clone(), join(OuterJoin::Left, "Q", 3)).is_none());
    assert!(applied(left_columns.clone(), join(OuterJoin::Full, "P", 3)).is_none());
    let right = LogicalPlan::OuterJoin { kind: OuterJoin::Right, cond: on(3), left: Box::new(scan("P")), right: Box::new(scan("Q")) };
    let Some(LogicalPlan::Project { exprs: mirrored, .. }) = applied(vec![Expr::Column(4)], right) else { panic!("mirrored") };
    assert_eq!(format!("{mirrored:?}"), format!("{:?}", [Expr::Column(1)]));
}
