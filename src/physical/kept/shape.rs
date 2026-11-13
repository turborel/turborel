//! \[DESIGN\] Whether two physical plans have one shape (RULES.md D45): the
//! same operators over the same tables, read the same way, by the same
//! indexes and probes, whatever values they hold. A prepared statement
//! planned with an execution's values, and planned again with its slots,
//! keeps the second plan once a run of executions gives both one shape
//! (`db/prepared/plans.rs`): the slots then stand where the values chose.
//!
//! Only the operators a kept plan may hold compare (`filled_plan`): a
//! table's scan, an index's, a filter, a projection, a grouping, DISTINCT,
//! a sort and an append. A join, a set operation, a system table or a
//! VALUES list has no shape a kept plan may take.

use crate::index::Probe;
use crate::physical::Plan;

/// Whether `one` and `other` have one shape, as the module says.
pub(crate) fn same_shape(one: &Plan, other: &Plan) -> bool {
    match one {
        Plan::SeqScan { table, columns, order, backward, .. } => {
            let Plan::SeqScan { table: table2, columns: columns2, order: order2, backward: backward2, .. } = other else { return false };
            table == table2 && columns == columns2 && order == order2 && backward == backward2
        }
        Plan::IndexScan { .. } => same_index_scan(one, other),
        Plan::Filter { input, .. } => {
            let Plan::Filter { input: input2, .. } = other else { return false };
            same_shape(input, input2)
        }
        Plan::Project { exprs, input, .. } => {
            let Plan::Project { exprs: exprs2, input: input2, .. } = other else { return false };
            exprs.len() == exprs2.len() && same_shape(input, input2)
        }
        Plan::Aggregate { groups, aggs, seeds, input, .. } => {
            let Plan::Aggregate { groups: groups2, aggs: aggs2, seeds: seeds2, input: input2, .. } = other else { return false };
            groups == groups2 && aggs.len() == aggs2.len() && seeds.len() == seeds2.len() && same_shape(input, input2)
        }
        Plan::Distinct { input } => {
            let Plan::Distinct { input: input2 } = other else { return false };
            same_shape(input, input2)
        }
        Plan::Sort { keys, input, .. } => {
            let Plan::Sort { keys: keys2, input: input2, .. } = other else { return false };
            keys == keys2 && same_shape(input, input2)
        }
        Plan::Append { left, right } => {
            let Plan::Append { left: left2, right: right2 } = other else { return false };
            same_shape(left, left2) && same_shape(right, right2)
        }
        Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::WorkTable { .. } => false,
    }
}

/// Whether `one` and `other` are scans of one index, read one way, by
/// probes of one shape (`same_probe`).
fn same_index_scan(one: &Plan, other: &Plan) -> bool {
    let Plan::IndexScan { table, index, probe, reverse, columns, covering, .. } = one else { return false };
    let Plan::IndexScan { table: table2, index: index2, probe: probe2, reverse: reverse2, columns: columns2, covering: covering2, .. } =
        other
    else {
        return false;
    };
    let read = table == table2 && index == index2 && reverse == reverse2;
    read && columns == columns2 && covering == covering2 && same_probe(probe, probe2)
}

/// Whether `one` and `other` are probes of one kind, holding as many
/// values equal and comparing the rest by the same operators.
fn same_probe(one: &Probe, other: &Probe) -> bool {
    match one {
        Probe::All => matches!(other, Probe::All),
        Probe::Compare { op, .. } => {
            let Probe::Compare { op: op2, .. } = other else { return false };
            op == op2
        }
        Probe::Prefix { equal, then } => {
            let Probe::Prefix { equal: equal2, then: then2 } = other else { return false };
            equal.len() == equal2.len() && then.as_ref().map(|(op, _)| *op) == then2.as_ref().map(|(op, _)| *op)
        }
        Probe::Range { equal, low, high } => {
            let Probe::Range { equal: equal2, low: low2, high: high2 } = other else { return false };
            equal.len() == equal2.len() && low.0 == low2.0 && high.0 == high2.0
        }
    }
}

// Unit tests: plans of one shape and of two (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::logical::SortKey;
    use crate::value::{CompOp, Decimal, SortOrder, Value};

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// A scan of table `T` by index `index`, `probe`, read `reverse`ly if so.
    fn scan(index: &str, probe: Probe, reverse: bool) -> Plan {
        Plan::IndexScan {
            table: QualifiedName { auth: "A".into(), ident: "T".into() },
            index: index.into(),
            method: String::new(),
            probe,
            slots: Vec::new(),
            reverse,
            columns: vec![0],
            detail: String::new(),
            est: 1.0_f64,
            cost: 1.0_f64,
            covering: false,
        }
    }

    /// A range probe of `low` to `high`, its upper bound by `op`.
    fn range(low: i128, high: i128, op: CompOp) -> Probe {
        Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, int(low))), high: Box::new((op, int(high))) }
    }

    /// Verifies: [DESIGN] RULES.md D45: scans of one index by probes of one
    /// kind and operators have one shape, whatever their values; another
    /// index, direction, kind of probe, operator or count of equal values
    /// each alone makes two.
    #[test]
    fn index_scans_have_one_shape_whatever_their_values() {
        let one = scan("I", range(1, 5, CompOp::Le), false);
        assert!(same_shape(&one, &scan("I", range(7, 9, CompOp::Le), false)));
        assert!(!same_shape(&one, &scan("J", range(1, 5, CompOp::Le), false)), "another index");
        assert!(!same_shape(&one, &scan("I", range(1, 5, CompOp::Le), true)), "backward");
        assert!(!same_shape(&one, &scan("I", range(1, 5, CompOp::Lt), false)), "another operator");
        assert!(!same_shape(&one, &scan("I", Probe::Compare { op: CompOp::Le, value: int(5) }, false)), "another kind");
        let prefix = |equal: Vec<Value>| scan("I", Probe::Prefix { equal, then: Some((CompOp::Lt, int(3))) }, false);
        assert!(same_shape(&prefix(vec![int(1)]), &prefix(vec![int(2)])));
        assert!(!same_shape(&prefix(vec![int(1)]), &prefix(vec![int(1), int(2)])), "more values equal");
        assert!(same_shape(&scan("I", Probe::All, false), &scan("I", Probe::All, false)));
    }

    /// Verifies: [DESIGN] RULES.md D45: operators over plans of one shape
    /// have one if they are alike: a sort by the same keys, a grouping by
    /// the same columns; a sort by others has two; a join has none.
    #[test]
    fn operators_over_one_shape_have_it_if_alike() {
        let sorted = |descending: bool| Plan::Sort {
            keys: vec![SortKey::new(0, SortOrder::of(descending, None))],
            detail: String::new(),
            input: Box::new(scan("I", Probe::All, false)),
        };
        assert!(same_shape(&sorted(false), &sorted(false)));
        assert!(!same_shape(&sorted(false), &sorted(true)), "another order");
        let distinct = Plan::Distinct { input: Box::new(sorted(false)) };
        assert!(same_shape(&distinct, &Plan::Distinct { input: Box::new(sorted(false)) }));
        assert!(!same_shape(&distinct, &sorted(false)), "another operator");
        let joined = Plan::Product { left: Box::new(sorted(false)), right: Box::new(sorted(false)) };
        assert!(!same_shape(&joined, &joined), "a join is kept by no plan");
    }
}
