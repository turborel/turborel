//! The mutations of set-shaped access on a key planned as index probes
//! (RULES.md R-TEST-9, D773 to D779): a subquery of many rows made a semi
//! join; two empty grouping sets taken as one row; the right's keys probed
//! without DISTINCT; a semi join's probes taken whether chosen or not; a
//! null bound probing the equalities alone; a bound's constant left out;
//! a bound's range not priced; a null collection taken as elements; the
//! object rows of UPDATE and DELETE planned without semi joins; a product
//! taken as one row by its left; a filter over a product judged untyped;
//! a range join priced against no probe of fixed columns; and a lateral
//! derived table bound as a derived table, one of nothing before it bound
//! laterally, its derived column list or its FETCH left out; a lateral's
//! collection of one row taken as raising, and COLLECT of one row too.

use super::{Mutation, Package};

/// The file of comparisons with a subquery of one row made semi joins.
const SCALAR: &str = "src/optimizer/unnesting/scalar.rs";
/// The file of a plan of one row at most.
const SINGLE: &str = "src/logical/single.rs";
/// The file of a semi join probing from the right.
const PROBED: &str = "src/physical/semi/probed.rs";
/// The file of an index join's probe bound of each outer row.
const JOIN_PROBE: &str = "src/physical/join_probe.rs";
/// The file of an index join's bounds chosen.
const BOUNDED: &str = "src/physical/indexed/bounded.rs";
/// The file of the estimates of each node.
const NODES: &str = "src/physical/cardinality/nodes.rs";
/// The file of the plans of statements compiled.
const COMPILED: &str = "src/db/compiled.rs";
/// The file of the optimizer's rules.
const OPTIMIZER: &str = "src/optimizer.rs";
/// The file of FROM's table references bound.
const FROM: &str = "src/binder/from.rs";
/// The file of a lateral derived table bound.
const LATERAL: &str = "src/binder/subquery/lateral.rs";
/// The file of what unnesting asks of the plans it moves.
const PURE: &str = "src/optimizer/unnesting/pure.rs";
/// The file of the multiset set functions.
const MULTISETS: &str = "src/functions/multisets.rs";
/// The file of the declared types of the columns an index join reads.
const TYPED: &str = "src/physical/indexed/typed.rs";
/// The file of a plan's slots filled and its outer references fixed.
const DYNAMIC: &str = "src/logical/dynamic.rs";
/// The unit test of a plan of one row at most.
const ONE_ROW: &str = "logical::single::tests::plans_of_one_row_at_most";
/// The unit test of an UNNEST's estimate.
const UNNESTED: &str = "physical::cardinality::tests::an_unnest_of_constants_is_estimated_by_their_elements";
/// The unit test of a lateral of one row estimated.
const LATERAL_ONE: &str = "physical::cardinality::tests::a_lateral_of_one_row_is_estimated_at_one_for_each_row_before_it";
/// The test of a key compared with a subquery.
const COMPARED: &str = "set_probes::a_key_compared_with_a_subquery_of_one_row_is_probed";
/// The test of a few keys in a set.
const IN_A_SET: &str = "set_probes::a_few_keys_in_a_set_probe_the_index";
/// The test of a join's bounds.
const RANGE: &str = "set_probes::a_joins_bounds_from_the_outer_row_probe_a_range";
/// The test of DELETE and UPDATE.
const CHANGED: &str = "set_probes::deletes_and_updates_of_a_few_keys_probe_them";
/// The test of a lateral query's rows.
const EACH_ROW: &str = "lateral::a_lateral_query_is_computed_for_each_row_before_it";
/// The test of a lateral set function probing the key.
const ONE_ENTRY: &str = "lateral::a_lateral_set_function_of_a_key_reads_one_entry_for_each_row_before_it";
/// The test of a lateral of a LEFT JOIN and of nothing before it.
const OF_NOTHING: &str = "lateral::a_lateral_of_a_left_join_keeps_its_rows_and_one_of_nothing_before_it_is_a_derived_table";
/// The test of a procedure's variables in the subqueries of its changes.
const PROCEDURES: &str = "set_probes::a_procedures_arrays_and_parameters_probe_the_key_of_its_changes";

/// See the module doc.
pub const SET_PROBES: [Mutation; 25] = [
    Mutation {
        name: "scalar-semi-of-many-rows",
        package: Package::Core,
        breaks: "a comparison with a subquery that may give more rows than one is a semi join, which raises no cardinality error",
        file: SCALAR,
        original: "if !sub.outer.is_empty() || !sub.plan.one_row_at_most() || raises",
        replacement: "if !sub.outer.is_empty() || raises",
        integration: &[COMPARED],
        unit: &[],
    },
    Mutation {
        name: "one-row-of-grouping-sets",
        package: Package::Core,
        breaks: "an aggregate of two empty grouping sets is taken as one row at most",
        file: SINGLE,
        original: "groups.is_empty() && seeds.len() <= 1",
        replacement: "groups.is_empty() && seeds.len() <= 2",
        integration: &[],
        unit: &[ONE_ROW],
    },
    Mutation {
        name: "semi-probes-without-distinct",
        package: Package::Core,
        breaks: "a semi join probes from each right row, a key named twice giving its left row twice",
        file: PROBED,
        original: "Some((LogicalPlan::Distinct { input: Box::new(keys) }, on))",
        replacement: "Some((keys, on))",
        integration: &[IN_A_SET],
        unit: &[],
    },
    Mutation {
        name: "semi-probes-unchosen",
        package: Package::Core,
        breaks: "a semi join of many right rows is an inner join of their keys whether it probes the index or not",
        file: PROBED,
        original: "probes(&built.plan).then(|| Built::unordered(built.plan))",
        replacement: "Some(Built::unordered(built.plan))",
        integration: &[IN_A_SET],
        unit: &[],
    },
    Mutation {
        name: "range-null-bound-skipped",
        package: Package::Core,
        breaks: "a null bound of an outer row is left out of its probe, which finds every key of the equalities",
        file: JOIN_PROBE,
        original: "let Some(bound) = b.bound(row)? else { return Ok(None) };",
        replacement: "let Some(bound) = b.bound(row)? else { continue };",
        integration: &[RANGE],
        unit: &[],
    },
    Mutation {
        name: "range-shift-left-out",
        package: Package::Core,
        breaks: "a bound of an outer column less a constant probes by the column alone",
        file: JOIN_PROBE,
        original: "Some(e) => crate::exec::eval(e, &vec![value], &crate::exec::NoSubqueries)?,",
        replacement: "Some(_) => value,",
        integration: &[RANGE],
        unit: &[],
    },
    Mutation {
        name: "range-unpriced",
        package: Package::Core,
        breaks: "a probe's bounds keep every row of its equalities in its price, so a hash join is chosen over it",
        file: BOUNDED,
        original: "let each = probing.each * share;",
        replacement: "let each = probing.each;",
        integration: &[RANGE],
        unit: &[],
    },
    Mutation {
        name: "unnest-null-has-elements",
        package: Package::Core,
        breaks: "a null collection of an UNNEST is estimated as holding elements",
        file: NODES,
        original: "value.is_null().then_some(0)",
        replacement: "value.is_null().then_some(5)",
        integration: &[],
        unit: &[UNNESTED],
    },
    Mutation {
        name: "written-rows-no-semi-joins",
        package: Package::Core,
        breaks: "the object rows of a searched UPDATE or DELETE are planned with IN and EXISTS nested, run for each row",
        file: COMPILED,
        original: "const WRITTEN_ACCESS_OFF: &[&str] = &[MARK_JOINS, UNNEST_SUBQUERIES, INLINE_FUNCTIONS];",
        replacement: "const WRITTEN_ACCESS_OFF: &[&str] = &[SEMI_JOINS, MARK_JOINS, UNNEST_SUBQUERIES, INLINE_FUNCTIONS];",
        integration: &[CHANGED],
        unit: &[],
    },
    Mutation {
        name: "one-row-of-product",
        package: Package::Core,
        breaks: "a product is taken as one row at most when its left operand is, whatever its right gives",
        file: SINGLE,
        original: "LogicalPlan::Product { left, right } => one_row(left, deeper) && one_row(right, deeper),",
        replacement: "LogicalPlan::Product { left, right: _ } => one_row(left, deeper),",
        integration: &[],
        unit: &[ONE_ROW],
    },
    Mutation {
        name: "product-filter-untyped",
        package: Package::Core,
        breaks: "a filter over a product with a bound `NXT - 20` is kept whole, its join a product",
        file: OPTIMIZER,
        original: "if cond.can_raise() && cond_can_raise_typed(cond, &|c| plan_column_type(input, c, catalog)) {",
        replacement: "if cond.can_raise() || cond_can_raise_typed(cond, &|c| plan_column_type(input, c, catalog)) {",
        integration: &[RANGE],
        unit: &[],
    },
    Mutation {
        name: "bounded-cost-unfixed",
        package: Package::Core,
        breaks: "a range join is priced against no probe of a key the inner table's filter fixes, and taken over it",
        file: BOUNDED,
        original: "let fixed = self.fixed_prefix(cond, outer, inner);",
        replacement: "let fixed = None::<super::fixed::Fixed>;",
        integration: &[COMPARED],
        unit: &[],
    },
    Mutation {
        name: "lateral-as-derived",
        package: Package::Core,
        breaks: "a lateral derived table is bound as a derived table, its references to the rows before it refused",
        file: FROM,
        original: "super::subquery::lateral_derived(b, d, start, from)?",
        replacement: "derived(b, d, start).or_at(d.query.at)?",
        integration: &[EACH_ROW],
        unit: &[],
    },
    Mutation {
        name: "lateral-of-nothing-unnested",
        package: Package::Core,
        breaks: "a lateral derived table that reads nothing before it is planned as an UNNEST, not as its derived table",
        file: LATERAL,
        original: "if reads.is_empty() {",
        replacement: "if reads.is_empty() && reads.len() > 1 {",
        integration: &[OF_NOTHING],
        unit: &[],
    },
    Mutation {
        name: "lateral-columns-unrenamed",
        package: Package::Core,
        breaks: "a lateral derived table's columns keep its query's names, its derived column list left out",
        file: LATERAL,
        original: "if let Some(names) = &d.columns {",
        replacement: "if let Some(names) = d.columns.as_ref().filter(|_| false) {",
        integration: &[EACH_ROW],
        unit: &[],
    },
    Mutation {
        name: "lateral-fetch-dropped",
        package: Package::Core,
        breaks: "a lateral query's OFFSET and FETCH are left out, every row of it kept",
        file: LATERAL,
        original: ".or_at(s.at)?.limited();",
        replacement: ".or_at(s.at)?;",
        integration: &[EACH_ROW],
        unit: &[],
    },
    Mutation {
        name: "lateral-collection-raises",
        package: Package::Core,
        breaks: "an UNNEST of a subquery of one row is taken as raising, so an IN of a lateral's MIN reads the whole table",
        file: PURE,
        original: "    if let Expr::Subquery(s, _) = e {",
        replacement: "    if let Some(s) = None::<&Subquery> {",
        integration: &[ONE_ENTRY],
        unit: &[],
    },
    Mutation {
        name: "lateral-collection-of-many",
        package: Package::Core,
        breaks: "COLLECT of many rows is taken as raising nothing, so an IN of a lateral of many rows is a semi join",
        file: PURE,
        original: "let once = groups.is_empty() && input.one_row_at_most();",
        replacement: "let once = groups.is_empty();",
        integration: &[ONE_ENTRY],
        unit: &[],
    },
    Mutation {
        name: "collect-of-one-raises",
        package: Package::Core,
        breaks: "COLLECT of one row is taken as raising, so a lateral's MIN in an IN is no semi join",
        file: MULTISETS,
        original: "self.kind == Gathering::Collect",
        replacement: "self.kind == Gathering::Collect && false",
        integration: &[ONE_ENTRY],
        unit: &[],
    },
    Mutation {
        name: "lateral-estimate-unknown",
        package: Package::Core,
        breaks: "an UNNEST of a lateral's one row is estimated at the default fanout of rows for each row before it",
        file: NODES,
        original: "Expr::Subquery(s, _) => return gathered_once(&s.plan).then_some(1),",
        replacement: "Expr::Subquery(..) => return None,",
        integration: &[],
        unit: &[LATERAL_ONE],
    },
    Mutation {
        name: "lateral-field-untyped",
        package: Package::Core,
        breaks: "a lateral's column has no known type, so an IN of its MIN probes no key of the table it compares",
        file: TYPED,
        original: "return row.fields.get(at).map(|f| f.data_type.clone());",
        replacement: "return row.fields.get(at).map(|f| f.data_type.clone()).filter(|_| false);",
        integration: &[ONE_ENTRY],
        unit: &[],
    },
    Mutation {
        name: "written-outer-unfixed",
        package: Package::Core,
        breaks: "a change's subqueries keep their outer references to a routine's variables, so they are decorrelated over the object table",
        file: COMPILED,
        original: "let logical = fixed_outer(logical);\n    let (optimized, trace) =",
        replacement: "let (optimized, trace) =",
        integration: &[PROCEDURES],
        unit: &[],
    },
    Mutation {
        name: "fixed-outer-kept",
        package: Package::Core,
        breaks: "a subquery whose outer references were all replaced keeps its outer expressions, so it is still taken as correlated",
        file: DYNAMIC,
        original: "if left { outer } else { Vec::new() }",
        replacement: "outer",
        integration: &[PROCEDURES],
        unit: &[],
    },
    Mutation {
        name: "fixed-outer-unreplaced",
        package: Package::Core,
        breaks: "an outer reference to a slot is left in the subquery's plan while its outer expression is dropped",
        file: DYNAMIC,
        original: "return fixed.get(i).cloned().flatten().unwrap_or(Expr::Outer(i, name));",
        replacement: "return Expr::Outer(i, name);",
        integration: &[PROCEDURES],
        unit: &[],
    },
    Mutation {
        name: "collection-constant-untyped",
        package: Package::Core,
        breaks: "a filled array has no declared type, so an IN of its UNNEST probes no key",
        file: TYPED,
        original: "return constant_type(value);",
        replacement: "return crate::exec::slot_type(value);",
        integration: &[PROCEDURES],
        unit: &[],
    },
];
