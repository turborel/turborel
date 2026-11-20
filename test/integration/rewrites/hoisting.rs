//! Parts of conditions over two operands that can raise, or are sort
//! keys, made columns of the operand each reads (RULES.md D373,
//! `optimizer/hoisting.rs`).

use turborel::optimizer::HOIST_RAISING_EXPRESSIONS;

use super::{COLUMNS, alike, compared, left, of_store, plan, rule, tables};

/// The note EXPLAIN gives a chain the enumeration ordered.
const ENUMERATED: &str = "(join order by DPccp: ";

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2]; [DESIGN] RULES.md D373,
/// D300: a chain written with a product first, whose conjunct over two
/// operands has a part that can raise of one, a quotient by a column, is
/// enumerated with the rule and not without it; a collated equality's
/// sort keys, made columns of their operands, are a hash join's keys; one of a
/// sum that cannot raise of its INTEGER columns, `A.V + 1`, is enumerated
/// either way; one whose raising part reads two, `A.V * C.X`, by neither.
/// Their rows are alike either way.
#[test]
fn hoisted_parts_let_chains_be_enumerated() {
    let (_db, s) = tables();
    let enumerated = "SELECT A.V, D.Y, C.X FROM {C} C, {A} A, {D} D WHERE A.K = D.K AND D.Y = C.K AND A.V / (A.K + 1000) < C.X";
    let collated = "SELECT A.V, B.W, C.X FROM {C} C, {A} A, {B} B WHERE A.K = B.K AND B.W = C.K AND A.S COLLATE CASE_INSENSITIVE = C.S";
    let query = of_store(enumerated, "");
    assert!(plan(&s, &query).contains(ENUMERATED), "{}", plan(&s, &query));
    rule(&s, HOIST_RAISING_EXPRESSIONS, false);
    assert!(!plan(&s, &query).contains(ENUMERATED), "{}", plan(&s, &query));
    rule(&s, HOIST_RAISING_EXPRESSIONS, true);
    alike(&s, HOIST_RAISING_EXPRESSIONS, enumerated);
    let keyed = alike(&s, HOIST_RAISING_EXPRESSIONS, collated);
    assert!(plan(&s, &of_store(collated, "")).contains("HashJoin A.S COLLATE CASE_INSENSITIVE"), "{keyed:?}");
    rule(&s, HOIST_RAISING_EXPRESSIONS, false);
    let sum = "SELECT A.V, B.W, C.X FROM A, B, C WHERE A.K = B.K AND B.W = C.K AND A.V + 1 > C.X";
    assert!(plan(&s, sum).contains(ENUMERATED), "{}", plan(&s, sum));
    rule(&s, HOIST_RAISING_EXPRESSIONS, true);
    let two = "SELECT A.V, D.Y, C.X FROM {A} A, {D} D, {C} C WHERE A.K = D.K AND D.Y = C.K AND A.V * C.X > 0";
    assert!(!plan(&s, &of_store(two, "")).contains(ENUMERATED));
    left(&s, HOIST_RAISING_EXPRESSIONS, two);
}

/// Verifies: [ISO/IEC 9075:1992 §7.11 GR1, §7.5 GR1c]; [DESIGN] RULES.md
/// D373, D313: a scalar subquery that may give more than one row, unnested
/// to a LEFT join of the chain and its `SINGLE`, has the join moved onto
/// the operand it reads and `SINGLE` hoisted onto that operand under a
/// guard: the rows and the error of more than one row are alike with the
/// rule and without it, of a three-table query.
#[test]
fn single_rows_of_scalar_subqueries_are_hoisted() {
    let (_db, s) = tables();
    let sql = "SELECT C.X, D.Y, Z.X FROM {C} C, {D} D, {Z} Z
        WHERE C.X = D.K AND Z.K = C.K AND C.K > (SELECT D2.Y FROM {D} D2 WHERE D2.K = C.X)";
    let found = alike(&s, HOIST_RAISING_EXPRESSIONS, sql);
    assert!(found.iter().all(|f| f.starts_with('[')), "{found:?}");
    let many = "SELECT C.X FROM {C} C, {D} D WHERE C.K = D.K AND C.X > (SELECT B2.W FROM {B} B2 WHERE B2.K = D.Y)";
    let raised = alike(&s, HOIST_RAISING_EXPRESSIONS, many);
    assert!(raised.iter().all(|r| r == "Cardinality"), "more than one row: {raised:?}");
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.9 GR5]; [DESIGN] RULES.md D373,
/// D15, R-TEST-7: a hoisted part that raises of a row of Z raises nothing
/// when another operand of the product is empty, and raises when it has a
/// row, though no row of it joins Z's; alike of an ON's part of the
/// operand that supplies nulls; a part of two operands still raises of a
/// pair the join would drop. Each with the rule on and off, of both
/// stores.
#[test]
fn hoisted_parts_raise_as_the_product_would() {
    let (_db, s) = tables();
    let quiet = [
        "SELECT Z.K FROM {Z} Z, {E} E WHERE Z.X / Z.Y > E.V",
        "SELECT Z.K FROM {Z} Z, {E} E, {D} D WHERE Z.X / Z.Y > E.V AND E.K = D.K",
        "SELECT E.K FROM {E} E LEFT JOIN {Z} Z ON E.K = Z.K AND Z.X / Z.Y > E.V",
    ];
    for sql in quiet {
        assert!(alike(&s, HOIST_RAISING_EXPRESSIONS, sql).iter().all(|r| r.starts_with("[]")), "{sql}");
    }
    let raising = [
        "SELECT Z.K FROM {Z} Z, {D} D WHERE Z.X / Z.Y > D.Y AND Z.K = D.K + 1000",
        "SELECT D.K FROM {D} D LEFT JOIN {Z} Z ON D.K = Z.K + 1000 AND Z.X / Z.Y > D.Y",
    ];
    for sql in raising {
        assert!(alike(&s, HOIST_RAISING_EXPRESSIONS, sql).iter().all(|r| r == "Data"), "{sql}");
    }
    let pair = "SELECT Z.K FROM {Z} Z, {D} D WHERE Z.K = D.K + 1000 AND D.Y / (Z.Y * D.K) > 0";
    for prefix in ["", COLUMNS] {
        assert_eq!(compared(&s, HOIST_RAISING_EXPRESSIONS, &of_store(pair, prefix)), "Data");
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; [DESIGN] RULES.md D373:
/// an ON's part of the preserved operand is hoisted only when it cannot
/// raise, a sort key or exact arithmetic within 18 digits, and the rows
/// of LEFT, RIGHT and FULL joins and of joins written with ON are alike
/// with the rule and without it.
#[test]
fn outer_joins_keep_their_rows_hoisted() {
    let (_db, s) = tables();
    let hoisted = [
        "SELECT C.K, D.Y FROM {C} C LEFT JOIN {D} D ON C.X = D.K AND C.X + 1 > D.Y",
        "SELECT C.K, D.Y FROM {C} C RIGHT JOIN {D} D ON C.X = D.K AND C.X + 1 > D.Y",
        "SELECT C.K, D.Y FROM {C} C FULL JOIN {D} D ON C.X = D.K AND C.X + 1 > D.Y",
        "SELECT C.K, Z.K FROM {C} C LEFT JOIN {Z} Z ON C.X = Z.K AND Z.X / Z.Y < C.K",
        "SELECT A.K, C.K FROM {A} A LEFT JOIN {C} C ON A.S COLLATE CASE_INSENSITIVE = C.S",
        "SELECT A.V, C.X FROM {A} A JOIN {B} B ON A.K = B.K JOIN {C} C ON B.W = C.K AND A.V + 1 > C.X",
    ];
    for sql in hoisted {
        alike(&s, HOIST_RAISING_EXPRESSIONS, sql);
    }
    let kept = [
        "SELECT Z.K, D.Y FROM {Z} Z LEFT JOIN {D} D ON Z.K = D.K AND Z.X / Z.Y > D.Y",
        "SELECT Z.K, D.Y FROM {Z} Z FULL JOIN {D} D ON Z.K = D.K AND Z.X / Z.Y > D.Y",
    ];
    for sql in kept {
        left(&s, HOIST_RAISING_EXPRESSIONS, sql);
    }
}
