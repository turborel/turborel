use super::*;
use crate::db::Database;
use crate::functions::Registry;
use crate::logical::AggCall;
use crate::value::Decimal;

/// The exact number `n`.
fn exact(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A grouping by its first column counting rows.
fn spec() -> Spec {
    let func = Registry::with_set_functions().aggregate("COUNT").unwrap();
    let result = func.0.result_type(None).unwrap();
    let count = AggCall { func, arg: None, distinct: false, result, filter: None, distribution: None, label: "COUNT".to_owned() };
    Spec { groups: vec![0], collations: Vec::new(), aggs: vec![count], seeds: Vec::new() }
}

/// The state of COUNT(*) of `rows` rows, as its accumulator writes it.
fn counted(spec: &Spec, rows: u64) -> Vec<u8> {
    let agg = spec.aggs.first().unwrap();
    let mut acc = agg.func.0.accumulator(agg.result.clone());
    for _ in 0..rows {
        acc.update(&Value::Null).unwrap();
    }
    let mut out = Vec::new();
    assert!(acc.write_state(&mut out));
    out
}

/// The groups the crossing gives, of 32 partitions each giving the groups
/// of the keys of 0 to 3999 whose rest of 8 is its own number's, so each
/// key's group comes of four, partition `p`'s group of key `k` first at
/// row `p * 10000 + k` of `p + 1` rows, kept at MEMORY.WORK_BYTES `work`:
/// in the order of their firsts, and whether any came from the spill.
/// Each partition's groups are fewer than the smallest grant holds, so
/// some are held before the rest pass it.
fn merged_at(work: i128) -> (Vec<String>, bool) {
    let db = Database::new();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(work, 0))).unwrap();
    let state = Db::from(db.snapshot().unwrap());
    let runner = Runner::new(&state, crate::txn::NO_TRANSACTION);
    let (spec, crossing) = (spec(), Crossing::new(Vec::new()));
    for p in 0_u64..32 {
        let groups = (0_u64..4000)
            .filter(|k| k.rem_euclid(8) == p.rem_euclid(8))
            .map(|k| (p.saturating_mul(10_000).saturating_add(k), vec![exact(k)], vec![counted(&spec, p.saturating_add(1))]));
        crossing.add(&spec, groups.collect(), &runner).unwrap();
    }
    let (held, given) = crossing.merged(&spec, Ok).unwrap().unwrap();
    let spilled = !given.is_empty();
    let mut all: Vec<(u64, Row)> = held.into_iter().flatten().collect();
    for run in given {
        all.extend(run.map(Result::unwrap));
    }
    all.sort_by_key(|g| g.0);
    (all.iter().map(|g| format!("{g:?}")).collect(), spilled)
}

/// Verifies: [DESIGN] (RULES.md D1228) the groups partitions give a
/// crossing second grouping, past the smallest MEMORY.WORK_BYTES, go to
/// the spill and are merged run by run, each group's counts summed and
/// numbered by its least first, as the groups held in memory at the
/// default grant are merged.
#[test]
fn groups_past_the_grant_merge_as_those_held() {
    let (held, spilled_held) = merged_at(256 << 20);
    let (small, spilled) = merged_at(64 << 10);
    assert!(!spilled_held, "held in memory at the default grant");
    assert!(spilled, "in the spill past the smallest grant");
    assert_eq!(held.len(), 4000);
    assert!(held.first().is_some_and(|g| g.contains("52")), "{:?}: 1 + 9 + 17 + 25 rows, the first first row 0", held.first());
    assert_eq!(small, held);
}
