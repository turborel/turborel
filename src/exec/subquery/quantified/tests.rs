use std::sync::Arc;

use super::super::super::eval;
use super::super::super::join::Rescanned;
use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::value::Decimal;

/// The values a test row's columns take: null, exact numbers, and an
/// approximate number equal to one of them.
fn value(n: u64) -> Value {
    match n.checked_rem(5).unwrap_or_default() {
        0 => Value::Null,
        1 => Value::Exact(Decimal::new(1, 0)),
        2 => Value::Exact(Decimal::new(2, 0)),
        3 => Value::Approx(2.0_f64),
        _ => Value::Exact(Decimal::new(3, 0)),
    }
}

/// Every row of `degree` values `value` gives.
fn every_row(degree: u32) -> Vec<Row> {
    (0..5_u64.pow(degree)).map(|n| (0..degree).map(|i| value(n.checked_div(5_u64.pow(i)).unwrap_or_default())).collect()).collect()
}

/// Asserts every op of ALL and SOME of `r` gives of `rows`, made of `s`,
/// what comparing `r` with each row of `s` gives.
fn answers_as_every_row(rows: &SubqueryRows, s: &[Row], r: &Row) {
    for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
        for q in [Quantifier::All, Quantifier::Some] {
            let expected = eval::quantified(r, (op, q), s.iter().cloned().map(Ok)).unwrap();
            assert_eq!(rows.quantified(r, (op, q)).unwrap(), expected, "{r:?} {op:?} {q:?} {s:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38, [ISO/IEC 9075:1992 §8.7 GR2, §8.2
/// GR7]: "R op ALL S" and "R op SOME S", answered from summaries, for every
/// op, every row R of one to three values, nulls among
/// them or not, and results S of up to six rows drawn alike, give what
/// comparing R with each row of S gives (`eval::quantified`).
#[test]
fn summaries_answer_as_every_row_does() {
    let db = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    let mut seed = 7_u64;
    for degree in 1_u32..=3_u32 {
        let rs = every_row(degree);
        for _ in 0_u32..60_u32 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let size = usize::try_from((seed >> 33).checked_rem(7).unwrap_or_default()).unwrap_or_default();
            let s: Vec<Row> = (0..size)
                .filter_map(|i| {
                    rs.get(
                        usize::try_from((seed >> 13).wrapping_add(u64::try_from(i).unwrap_or_default().wrapping_mul(977)))
                            .unwrap_or_default()
                            .checked_rem(rs.len())
                            .unwrap_or_default(),
                    )
                    .cloned()
                })
                .collect();
            let rows = SubqueryRows::of(Rescanned::Held(Arc::new(s.clone())), &db);
            for r in &rs {
                answers_as_every_row(&rows, &s, r);
            }
        }
    }
}
