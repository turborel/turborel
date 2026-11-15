// Unit tests: the closure, each rule over plans of each node, and the
// conditions they take, pairs of which show MC/DC (RULES.md R-TEST-1,
// R-TEST-8).

use std::sync::Arc;

use super::*;
use crate::catalog::{Catalog, ColumnDesc, ConstraintAttributes, ConstraintDesc, ConstraintKind, MemoryCatalog, QualifiedName, TableDesc};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{OuterJoin, Subquery};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// A column `name` of INTEGER, NOT NULL if `not_null`.
fn column(name: &str) -> ColumnDesc {
    let data_type = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    ColumnDesc { name: name.into(), data_type, not_null: false, default: None, domain: None, collation: None, generated: None }
}

/// A constraint `kind`, deferrable if `deferrable`.
fn constraint(kind: ConstraintKind, deferrable: bool) -> ConstraintDesc {
    let attributes = ConstraintAttributes { deferrable, initially_deferred: false };
    ConstraintDesc { name: QualifiedName { auth: "A".into(), ident: "C".into() }, kind, attributes, domain: None }
}

/// A UNIQUE of `columns`, PRIMARY KEY if `primary`, of nulls distinct if
/// `nulls_distinct`.
fn unique(columns: Vec<usize>, (primary, nulls_distinct): (bool, bool)) -> ConstraintKind {
    ConstraintKind::Unique { columns, primary, nulls_distinct }
}

/// A database of table `A.T` of columns K, V and W, of `constraints`.
fn database(constraints: Vec<ConstraintDesc>) -> State {
    let mut catalog = MemoryCatalog::default();
    catalog.add_table(TableDesc {
        name: QualifiedName { auth: "A".into(), ident: "T".into() },
        columns: vec![column("K"), column("V"), column("W")],
        constraints,
        engine: "ROW".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    });
    State::with_builtins(Box::new(catalog))
}

/// A scan of `A.T`.
fn scan() -> LogicalPlan {
    LogicalPlan::Scan {
        table: QualifiedName { auth: "A".into(), ident: "T".into() },
        names: vec!["K".into(), "V".into(), "W".into()],
        needed: vec![],
        located: false,
    }
}

fn one() -> Expr {
    Expr::Const(Value::Exact(Decimal::new(1, 0)))
}

fn equal(a: Expr, b: Expr) -> Cond {
    Cond::Compare(a, CompOp::Eq, b)
}

fn and(a: Cond, b: Cond) -> Cond {
    Cond::And(Box::new(a), Box::new(b))
}

/// What is known of `plan` of `db`.
fn of(db: &State, plan: &LogicalPlan) -> Known {
    Known::of(db, plan)
}

/// A function that is not deterministic.
struct Dice;
impl ScalarFunction for Dice {
    fn name(&self) -> &'static str {
        "DICE"
    }
    fn return_type(&self, args: &[DataType]) -> crate::error::Result<DataType> {
        Ok(args.first().cloned().unwrap_or(DataType::Boolean))
    }
    fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
        Ok(args.first().cloned().unwrap_or(Value::Null))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

fn dice(e: Expr) -> Expr {
    Expr::Call(ScalarRef(Arc::new(Dice)), vec![e])
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.1] the closure applies
/// the rules of deduction until nothing is added, chains through an
/// attribute no column has, takes no axiomatic dependency, and a
/// determinant out of range determines nothing more.
#[test]
fn the_closure_chains_dependencies() {
    let mut k = Known { columns: vec![0, 1, 2, 3], ..Known::default() };
    k.depends(Attrs::from([0]), Attrs::from([9]));
    k.depends(Attrs::from([9, 1]), Attrs::from([2]));
    k.depends(Attrs::from([2]), Attrs::from([2]));
    assert_eq!(k.dependencies.len(), 2, "the axiomatic one is not kept");
    assert!(k.determines(&[0, 1], 2));
    assert!(!k.determines(&[0], 2), "9 alone does not give 2");
    assert!(!k.determines(&[1], 2));
    assert!(k.determines(&[3], 3), "a column determines itself");
    assert!(!k.determines(&[0, 1, 7], 3));
    assert!(!k.determines(&[0, 1], 7), "no such column");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.3] of a base table, a
/// non-deferrable UNIQUE determines every column if it is the PRIMARY KEY,
/// NULLS NOT DISTINCT, or of columns known not null, each condition alone
/// deciding (MC/DC); a deferrable one, and one of a column only deferrably
/// NOT NULL, do not; NOT NULL is known of a non-deferrable constraint.
#[test]
fn a_base_tables_keys_determine_its_rows() {
    let not_null = |deferrable: bool| constraint(ConstraintKind::NotNull(0), deferrable);
    let cases = [
        (vec![constraint(unique(vec![0], (true, true)), false)], true),
        (vec![constraint(unique(vec![0], (false, true)), false)], false),
        (vec![constraint(unique(vec![0], (false, false)), false)], true),
        (vec![constraint(unique(vec![0], (false, true)), false), not_null(false)], true),
        (vec![constraint(unique(vec![0], (false, true)), false), not_null(true)], false),
        (vec![constraint(unique(vec![0], (true, true)), true)], false),
    ];
    for (n, (constraints, determines)) in cases.into_iter().enumerate() {
        let db = database(constraints);
        let k = of(&db, &scan());
        assert_eq!(k.determines(&[0], 1) && k.determines(&[0], 2), determines, "case {n}");
    }
    let k = of(&database(vec![constraint(ConstraintKind::NotNull(1), false)]), &scan());
    assert_eq!(k.not_null, Attrs::from([k.columns[1]]));
    let missing = LogicalPlan::Scan {
        table: QualifiedName { auth: "A".into(), ident: "GONE".into() },
        names: vec!["X".into()],
        needed: vec![],
        located: false,
    };
    assert_eq!(of(&database(vec![]), &missing).dependencies, Vec::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.12] of an equality
/// AND-component, two columns determine each other, a constant determines
/// a column, a deterministic expression of columns determines it and one
/// not deterministic does not; an OR is no AND-component; each column
/// compared, or IS NOT NULL, is known not null.
#[test]
fn equalities_of_a_condition_determine_columns() {
    let db = database(vec![]);
    let filtered = |cond: Cond| of(&db, &LogicalPlan::Filter { cond, input: Box::new(scan()) });
    let both = filtered(equal(Expr::Column(0), Expr::Column(1)));
    assert!(both.determines(&[0], 1) && both.determines(&[1], 0) && !both.determines(&[0], 2));
    let constant = filtered(and(equal(Expr::Column(2), one()), Cond::IsNull(Expr::Column(1), true)));
    assert!(constant.determines(&[], 2));
    assert_eq!(constant.not_null, Attrs::from([constant.columns[1], constant.columns[2]]));
    let computed = filtered(equal(Expr::Neg(Box::new(Expr::Column(0))), Expr::Column(1)));
    assert!(computed.determines(&[0], 1) && !computed.determines(&[1], 0));
    assert!(!filtered(equal(dice(Expr::Column(0)), Expr::Column(1))).determines(&[0], 1));
    let or = Cond::Or(Box::new(equal(Expr::Column(2), one())), Box::new(equal(Expr::Column(2), one())));
    assert!(!filtered(or).determines(&[], 2));
    let less = filtered(Cond::Compare(Expr::Column(0), CompOp::Lt, one()));
    assert!(less.not_null.contains(&less.columns[0]) && less.dependencies.is_empty());
}

/// The outer join `kind` of a scan of `db` and a scan of it, on `cond`.
fn outer(db: &State, kind: OuterJoin, cond: Cond) -> Known {
    of(db, &LogicalPlan::OuterJoin { kind, cond, left: Box::new(scan()), right: Box::new(scan()) })
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.7] an outer join keeps
/// its preserved operand's dependencies; of an operand extended with
/// nulls, only one whose determinant has a column known not null, of its
/// own columns; FULL keeps neither but those.
#[test]
fn an_outer_join_keeps_what_nulls_cannot_break() {
    let keyed = database(vec![constraint(unique(vec![0], (true, true)), false)]);
    let loose = database(vec![constraint(unique(vec![0], (false, false)), false)]);
    let none = Cond::Const(crate::logical::Truth::True);
    for (kind, left, right) in [(OuterJoin::Left, true, true), (OuterJoin::Right, true, true), (OuterJoin::Full, true, true)] {
        let k = outer(&keyed, kind, none.clone());
        assert_eq!((k.determines(&[0], 1), k.determines(&[3], 4)), (left, right), "{kind:?} of a key not null");
    }
    for (kind, left, right) in [(OuterJoin::Left, true, false), (OuterJoin::Right, false, true), (OuterJoin::Full, false, false)] {
        let k = outer(&loose, kind, none.clone());
        assert_eq!((k.determines(&[0], 1), k.determines(&[3], 4)), (left, right), "{kind:?} of a nullable key");
    }
    assert!(outer(&keyed, OuterJoin::Full, none).not_null.is_empty());
    // A dependency of an attribute no column has is not kept of an
    // operand extended with nulls.
    let mut right = Known { columns: vec![0, 1], not_null: Attrs::from([0, 1]), ..Known::default() };
    right.depends(Attrs::from([0]), Attrs::from([7]));
    right.depends(Attrs::from([7, 0]), Attrs::from([1]));
    right.depends(Attrs::from([8]), Attrs::from([1]));
    right.depends(Attrs::from([0]), Attrs::from([1]));
    assert_eq!(right.null_extended().dependencies, vec![(Attrs::from([0]), Attrs::from([1]))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.7] of a LEFT join on a
/// deterministic condition, the preserved columns it references determine a
/// column of the other an equality compares with a constant or a preserved
/// column, not with a column of its own side, nor of a condition not
/// deterministic; that column is null with the preserved one; RIGHT is the
/// mirror; FULL adds nothing.
#[test]
fn a_left_joins_condition_determines_the_other_side() {
    let db = database(vec![]);
    let left = |cond: Cond| outer(&db, OuterJoin::Left, cond);
    let both = left(and(equal(Expr::Column(0), Expr::Column(4)), equal(Expr::Column(5), one())));
    assert!(both.determines(&[0], 4) && both.determines(&[0], 5) && !both.determines(&[0], 3));
    assert_eq!(both.nulled_with, vec![(both.columns[0], both.columns[4])]);
    assert!(!left(equal(Expr::Column(3), Expr::Column(4))).determines(&[0], 4), "both of the other side");
    let undetermined = and(equal(Expr::Column(0), Expr::Column(4)), Cond::Value(dice(Expr::Column(1))));
    let chance = left(undetermined);
    assert!(!chance.determines(&[0, 1], 4) && chance.nulled_with.is_empty());
    let right = outer(&db, OuterJoin::Right, equal(Expr::Column(4), Expr::Column(0)));
    assert!(right.determines(&[4], 0) && !right.determines(&[0], 4));
    assert!(!outer(&db, OuterJoin::Full, equal(Expr::Column(0), Expr::Column(4))).determines(&[0], 4));
}

/// The COALESCE of columns `x` and `y`, as a NATURAL join's.
fn coalesce(x: usize, y: usize) -> Expr {
    Expr::Case(vec![(Cond::IsNull(Expr::Column(x), true), Expr::Column(x))], Box::new(Expr::Column(y)))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.15, §4.26.7] of a
/// projection, a column reference keeps its attribute, a deterministic
/// expression is determined by its columns, one not deterministic or of a
/// subquery is not; a COALESCE of x is alike x where x is known not null or
/// the other is null with it, not otherwise.
#[test]
fn a_projection_keeps_and_computes() {
    let db = database(vec![constraint(unique(vec![0], (true, true)), false)]);
    let project = |exprs: Vec<Expr>, input: LogicalPlan| of(&db, &LogicalPlan::Project { exprs, input: Box::new(input) });
    let neg = Expr::Neg(Box::new(Expr::Column(1)));
    let sub = Expr::Subquery(Arc::new(Subquery { plan: scan(), outer: vec![] }), 0);
    let k = project(vec![Expr::Column(0), neg, dice(Expr::Column(1)), sub], scan());
    assert!(k.determines(&[0], 1) && !k.determines(&[0], 2) && !k.determines(&[0], 3));
    let alike = project(vec![coalesce(0, 1), Expr::Column(2)], scan());
    assert!(alike.determines(&[0], 1), "of a column not null");
    let nullable = project(vec![coalesce(1, 0), Expr::Column(1)], scan());
    assert!(!nullable.determines(&[0], 1), "of a nullable column");
    let joined = LogicalPlan::OuterJoin {
        kind: OuterJoin::Left,
        cond: equal(Expr::Column(1), Expr::Column(3)),
        left: Box::new(scan()),
        right: Box::new(scan()),
    };
    let nulled = project(vec![coalesce(1, 3), Expr::Column(1)], joined);
    assert!(nulled.determines(&[0], 1), "null with it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.13, §4.26.16, §4.26.6,
/// §4.26.8] grouping columns determine the set functions' and keep what is
/// known of the rows, unless groups of no rows are made; UNION knows
/// nothing, EXCEPT its first operand's, INTERSECT both's; a table value
/// constructor of one row determines all, of two nothing; an ordinality
/// column with the input's determines a collection's elements, if its
/// collections are deterministic.
#[test]
fn groups_set_operations_and_collections() {
    let db = database(vec![constraint(unique(vec![0], (true, true)), false)]);
    let aggregate = |seeds| LogicalPlan::Aggregate { groups: vec![1, 0], collations: vec![], aggs: vec![], seeds, input: Box::new(scan()) };
    let groups = of(&db, &aggregate(vec![]));
    assert!(groups.determines(&[1], 0), "the key of the rows, second, determines the first");
    assert!(!of(&db, &aggregate(vec![vec![Value::Null, Value::Null]])).determines(&[1], 0));
    let set = |op| of(&db, &LogicalPlan::SetOp { op, all: false, compared: None, left: Box::new(scan()), right: Box::new(scan()) });
    assert!(!set(SetOp::Union).determines(&[0], 1));
    assert!(set(SetOp::Except).determines(&[0], 1));
    let loose = database(vec![]);
    let product = LogicalPlan::Product { left: Box::new(scan()), right: Box::new(scan()) };
    let pairs = LogicalPlan::Project { exprs: vec![Expr::Column(0), Expr::Column(4)], input: Box::new(product) };
    let keyed = LogicalPlan::Project { exprs: vec![Expr::Column(0), Expr::Column(1)], input: Box::new(scan()) };
    let meet = |l: &LogicalPlan, r: &LogicalPlan, d: &State| {
        of(
            d,
            &LogicalPlan::SetOp { op: SetOp::Intersect, all: false, compared: None, left: Box::new(l.clone()), right: Box::new(r.clone()) },
        )
    };
    assert!(meet(&pairs, &keyed, &db).determines(&[0], 1), "the second's");
    assert!(!meet(&pairs, &keyed, &loose).determines(&[0], 1));
    let values = |rows: usize| of(&db, &LogicalPlan::Values { rows: vec![vec![one(), one()]; rows], types: vec![DataType::Boolean; 2] });
    assert!(values(1).determines(&[], 1) && !values(2).determines(&[], 1));
    let unnest = |ordinality: bool, collection: Expr| {
        let u = Unnest { collections: vec![collection], spread: vec![None], ordinality, outer: None, names: vec![], input: scan() };
        of(&loose, &LogicalPlan::Unnest(Box::new(u)))
    };
    let ordered = unnest(true, Expr::Column(1));
    assert!(ordered.determines(&[0, 1, 2, 4], 3) && !ordered.determines(&[4], 3) && ordered.not_null.contains(&ordered.columns[4]));
    assert!(!unnest(false, Expr::Column(1)).determines(&[0, 1, 2], 3));
    assert!(!unnest(true, dice(Expr::Column(1))).determines(&[0, 1, 2, 4], 3));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.27] the left-most rule: of
/// two sets of columns, in order of their numbers, the shorter extended
/// with zeros, the first that differ the smaller; alike, neither.
#[test]
fn the_left_most_rule_orders_sets() {
    assert!(keys::left_of(&[0, 2], &[1]), "1 before 2");
    assert!(!keys::left_of(&[1], &[0, 2]));
    assert!(keys::left_of(&[0], &[0, 1]), "a zero after 1");
    assert!(keys::left_of(&[], &[0]));
    assert!(!keys::left_of(&[0, 1], &[0, 1]), "alike");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.27] the search finds the
/// left-most candidate key, {1, 3}, before {2}, which leaving the
/// right-most columns out first would find; and stops after its limit.
#[test]
fn the_search_finds_the_left_most_candidate_key() {
    let mut k = Known { columns: vec![0, 1, 2], ..Known::default() };
    k.depends(Attrs::from([0, 2]), Attrs::from([1]));
    k.depends(Attrs::from([1]), Attrs::from([0, 2]));
    let mut budget = 64;
    assert_eq!(k.leftmost(&[0, 1, 2], &mut budget), keys::Found::Key(vec![0, 2]));
    assert_eq!(k.leftmost(&[1, 2], &mut budget), keys::Found::Key(vec![1]));
    assert_eq!(k.leftmost(&[2], &mut budget), keys::Found::None, "no key within column 3 alone");
    let mut spent = 1;
    assert_eq!(k.leftmost(&[0, 1, 2], &mut spent), keys::Found::Stopped);
    let constant = Known { columns: vec![0], dependencies: vec![(Attrs::new(), Attrs::from([0]))], ..Known::default() };
    assert_eq!(constant.leftmost(&[0], &mut budget), keys::Found::Key(Vec::new()), "of at most one row");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.27, §4.26.3] of a base
/// table, its PRIMARY KEY is described, else a UNIQUE of a column not null,
/// else all its columns; a search stopped describes none.
#[test]
fn a_tables_key_is_described() {
    let primary = database(vec![constraint(unique(vec![1], (true, true)), false)]);
    assert_eq!(described_key(&primary, &scan()), DescribedKey::Primary(vec![1]));
    let not_null = constraint(ConstraintKind::NotNull(2), false);
    let unique = database(vec![constraint(unique(vec![2], (false, true)), false), not_null]);
    assert_eq!(described_key(&unique, &scan()), DescribedKey::Preferred(vec![2]));
    assert_eq!(described_key(&database(vec![]), &scan()), DescribedKey::Preferred(vec![0, 1, 2]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.27] of two candidate keys,
/// the left-most is preferred unless the other is strong, of a BUC-set or
/// within a grouped table's grouping columns, nullable as they may be.
#[test]
fn a_strong_candidate_key_is_preferred() {
    let mut k = Known { columns: vec![0, 1], ..Known::default() };
    k.same(0, 1);
    assert_eq!(k.described(), DescribedKey::Preferred(vec![0]));
    let grouped = Known { grouping: Some(Attrs::from([1])), ..k.clone() };
    assert_eq!(grouped.described(), DescribedKey::Preferred(vec![1]));
    let unique = Known { keys: vec![(Attrs::from([1]), false)], ..k };
    assert_eq!(unique.described(), DescribedKey::Preferred(vec![1]));
}
