//! \[DESIGN\] Whether two physical plans have one shape (RULES.md D45): the
//! same operators over the same tables, read the same way, by the same
//! indexes and probes, whatever values they hold. A prepared statement
//! planned with an execution's values, and planned again with its slots,
//! keeps the second plan once a run of executions gives both one shape
//! (`db/prepared/plans.rs`): the slots then stand where the values chose.
//!
//! Only the operators a kept plan may hold compare (`filled_plan`): a
//! table's scan, an index's, a filter, a projection, a grouping, DISTINCT,
//! a sort, a limit and an append. A join, a set operation, a system table or a
//! VALUES list has no shape a kept plan may take.

use crate::functions::ScalarRef;
use crate::index::Probe;
use crate::logical::Limit;
use crate::physical::Plan;
use std::sync::Arc;

/// Whether `one` and `other` have one shape, as the module says.
pub(crate) fn same_shape(one: &Plan, other: &Plan) -> bool {
    match one {
        Plan::SeqScan { .. } => same_scan(one, other),
        Plan::IndexScan { .. } => same_index_scan(one, other),
        Plan::Nearest(_) => same_nearest(one, other),
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
        Plan::Limit { limit, input } => {
            let Plan::Limit { limit: limit2, input: input2 } = other else { return false };
            let fetch = |l: &Limit| l.fetch.as_ref().map(|f| (f.percent, f.ties.clone()));
            limit.offset.is_some() == limit2.offset.is_some() && fetch(limit) == fetch(limit2) && same_shape(input, input2)
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
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. } => false,
    }
}

fn same_scan(one: &Plan, other: &Plan) -> bool {
    let Plan::SeqScan { table, columns, order, backward, .. } = one else { return false };
    let Plan::SeqScan { table: table2, columns: columns2, order: order2, backward: backward2, .. } = other else { return false };
    table == table2 && columns == columns2 && order == order2 && backward == backward2
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

fn same_nearest(one: &Plan, other: &Plan) -> bool {
    let Plan::Nearest(a) = one else { return false };
    let Plan::Nearest(b) = other else { return false };
    let (Some((column, argument, function, _)), Some((other_column, other_argument, other_function, _))) =
        (crate::physical::distance_call(&a.measure), crate::physical::distance_call(&b.measure))
    else {
        return false;
    };
    let source = a.table == b.table && a.index == b.index && a.columns == b.columns;
    let layout = a.located == b.located && a.width == b.width && a.first == b.first;
    let call = column == other_column && argument == other_argument && same_function(function, other_function);
    let approximate = a.approximate == b.approximate;
    source && layout && call && approximate && a.descending == b.descending && a.condition.is_some() == b.condition.is_some()
}

/// Whether `one` and `other` are probes of one kind, holding as many
/// values equal and comparing the rest by the same operators.
fn same_probe(one: &Probe, other: &Probe) -> bool {
    match one {
        Probe::All => matches!(other, Probe::All),
        Probe::Predicate { predicate: a, .. } => {
            let Probe::Predicate { predicate: b, .. } = other else { return false };
            same_function(&a.function, &b.function) && a.column == b.column && a.argument == b.argument
        }
        Probe::Columns(each) => {
            let Probe::Columns(those) = other else { return false };
            each.len() == those.len() && each.iter().zip(those).all(|(a, b)| a.column == b.column && a.op == b.op)
        }
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

/// Preparation changes object identity but preserves complete declared
/// spatial semantics. Ordinary predicates still require one object;
/// neither a shared SQL name nor two absent declarations establish this.
fn same_function(a: &ScalarRef, b: &ScalarRef) -> bool {
    if Arc::ptr_eq(&a.0, &b.0) {
        return true;
    }
    let Some(spatial) = a.0.spatial_operation() else { return false };
    a.0.deterministic() && b.0.deterministic() && b.0.spatial_operation() == Some(spatial)
}

// Unit tests: plans of one shape and of two (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::logical::SortKey;
    use crate::value::{CompOp, Decimal, SortOrder, Value};

    struct Declared {
        function: crate::geometry::GeometryFunction,
        deterministic: bool,
    }

    impl crate::functions::ScalarFunction for Declared {
        fn name(&self) -> &str {
            self.function.name()
        }

        fn return_type(&self, args: &[crate::value::DataType]) -> crate::error::Result<crate::value::DataType> {
            self.function.return_type(args)
        }

        fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
            self.function.call(args)
        }

        fn deterministic(&self) -> bool {
            self.deterministic
        }

        fn spatial_operation(&self) -> Option<crate::geometry::SpatialOperation> {
            self.function.spatial_operation()
        }
    }

    /// Verifies: [DESIGN] D45, D85: distinct function objects share a
    /// probe shape only through complete spatial semantics. MC/DC: each
    /// determinism declaration and each spatial declaration independently
    /// prevents this; ordinary predicates still require object identity.
    #[test]
    fn spatial_function_shapes_require_complete_deterministic_semantics() {
        use crate::geometry::GeometryFunction::{Distance, Intersects, X};
        let function = |function, deterministic| ScalarRef(Arc::new(Declared { function, deterministic }));
        let spatial = function(Intersects, true);
        assert!(same_function(&spatial, &function(Intersects, true)));
        assert!(!same_function(&function(Intersects, false), &spatial));
        assert!(!same_function(&spatial, &function(Intersects, false)));
        assert!(!same_function(&function(Distance, true), &spatial));
        assert!(!same_function(&spatial, &function(Distance, true)));
        let ordinary = function(X, true);
        assert!(!same_function(&ordinary, &function(X, true)));
        assert!(!same_function(&ordinary, &spatial));
        assert!(!same_function(&spatial, &ordinary));
        assert!(same_function(&ordinary, &ordinary.clone()));
    }

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
            located: false,
        }
    }

    /// A range probe of `low` to `high`, its upper bound by `op`.
    fn range(low: i128, high: i128, op: CompOp) -> Probe {
        Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, int(low))), high: Box::new((op, int(high))) }
    }

    /// Verifies: [DESIGN] D34, D45: plan reuse compares the registered
    /// function and the column's positions in the key and call, ignoring
    /// the other argument's value. MC/DC pairs change only the function,
    /// key column or call position; each independently changes the shape.
    #[test]
    fn predicate_shapes_keep_the_function_and_positions() {
        let registry = crate::functions::Registry::with_set_functions();
        let function = registry.scalar("UPPER").unwrap();
        let make = |bound, column, argument, value| Probe::Predicate {
            predicate: Box::new(crate::index::Predicate { function: bound, column, argument }),
            value,
        };
        let one = make(function.clone(), 0, 0, int(1));
        assert!(same_probe(&one, &make(function.clone(), 0, 0, Value::Null)));
        assert!(!same_probe(&one, &make(registry.scalar("LOWER").unwrap(), 0, 0, int(1))), "function");
        assert!(!same_probe(&one, &make(function.clone(), 1, 0, int(1))), "key column");
        assert!(!same_probe(&one, &make(function, 0, 1, int(1))), "argument position");
        assert!(!same_probe(&one, &Probe::All), "probe kind");
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
            most: None,
        };
        assert!(same_shape(&sorted(false), &sorted(false)));
        assert!(!same_shape(&sorted(false), &sorted(true)), "another order");
        let distinct = Plan::Distinct { input: Box::new(sorted(false)) };
        assert!(same_shape(&distinct, &Plan::Distinct { input: Box::new(sorted(false)) }));
        assert!(!same_shape(&distinct, &sorted(false)), "another operator");
        let joined = Plan::Product { left: Box::new(sorted(false)), right: Box::new(sorted(false)) };
        assert!(!same_shape(&joined, &joined), "a join is kept by no plan");
    }

    /// Verifies: [DESIGN] Independent probes share a prepared shape only
    /// when their comparison count, column positions and operators agree.
    /// MC/DC: changing each alone rejects reuse; changing values permits it.
    #[test]
    fn independent_probe_shapes_include_columns_and_operators() {
        use crate::index::ColumnComparison;
        let probe = |column, op, value| Probe::Columns(vec![ColumnComparison { column, op, value: int(value) }]);
        let one = probe(1, CompOp::Ge, 3);
        assert!(same_probe(&one, &probe(1, CompOp::Ge, 5)));
        assert!(!same_probe(&one, &probe(0, CompOp::Ge, 3)));
        assert!(!same_probe(&one, &probe(1, CompOp::Gt, 3)));
        assert!(!same_probe(&one, &Probe::Columns(Vec::new())));
        assert!(!same_probe(&one, &Probe::All));
    }
}
