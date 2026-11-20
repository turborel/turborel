//! The mutations of the OLTP paths (RULES.md R-TEST-9, D403 to D412): a
//! B-tree index made by CREATE INDEX holding no key with a null, the sort
//! an ORDER BY asks dropped where a null key survives a probe, and the
//! equality of two columns equal to one number or one `?` not added.

use super::super::{Mutation, Package};

/// Mechanisms of the OLTP paths whose removal must fail their tests.
pub const OLTP: [Mutation; 7] = [
    Mutation {
        name: "oltp-null-keys-not-held",
        package: Package::Core,
        breaks: "a B-tree index made by CREATE INDEX holds no key with a null, so a probe of its first columns is refused",
        file: "src/db/collations.rs",
        original: "        let nulls = nulls || (!ix.unique && ix.method == crate::index::BTREE_METHOD);",
        replacement: "        let nulls = nulls || (!ix.unique && ix.method.is_empty());",
        integration: &["planning::indexes_on_several_columns_answer_prefixes"],
        unit: &[],
    },
    Mutation {
        name: "oltp-held-nulls-ordered",
        package: Package::Core,
        breaks: "an index holding keys with a null gives the order of an ORDER BY, its nulls first",
        file: "src/physical/access/predicate.rs",
        original: "    untested_nulls(columns, probe) || (ix.nulls && untested())",
        replacement: "    untested_nulls(columns, probe) || (ix.nulls && untested() && false)",
        integration: &["predicate_indexes::nullable_candidates_keep_the_requested_null_order"],
        unit: &[],
    },
    Mutation {
        name: "oltp-equated-by-values-lost",
        package: Package::Core,
        breaks: "two columns of two tables equal to one number are not equated, so their join hashes",
        file: "src/optimizer/transitivity/joined.rs",
        original: "            if value == other && apart && new {",
        replacement: "            if value == other && apart && new && false {",
        integration: &["equated_joins::columns_equal_to_one_number_join_by_an_index"],
        unit: &[],
    },
    Mutation {
        name: "oltp-equated-slot-lost",
        package: Package::Core,
        breaks: "a column equal to a `?` of exact numeric type is not held by the `?`",
        file: "src/optimizer/transitivity/joined.rs",
        original: "    matches!(slot.data_type, DataType::Exact { .. }).then_some((column, Held::Slot(slot.index)))",
        replacement: "    matches!(slot.data_type, DataType::Boolean).then_some((column, Held::Slot(slot.index)))",
        integration: &[],
        unit: &["optimizer::transitivity::joined::tests::a_column_equal_to_an_exact_parameter_is_held_by_its_place"],
    },
    Mutation {
        name: "oltp-budget-recorded-over-given",
        package: Package::Core,
        breaks: "a database opened again makes its pool in the budget it recorded, not the one given as it opens",
        file: "src/db/files.rs",
        original: "snapshot.memory.set_limit(crate::settings::budget_bytes(values.get(crate::settings::MEMORY_BUDGET_BYTES)));",
        replacement: "snapshot.memory.set_limit(recorded(\"MEMORY.BUDGET_BYTES\").map_or(crate::memory::DEFAULT_BUDGET, |v| crate::settings::budget_bytes(&v)));",
        integration: &["creation::the_budget_given_as_the_database_opens_holds_its_pool"],
        unit: &[],
    },
    Mutation {
        name: "oltp-kept-body-never-stale",
        package: Package::Core,
        breaks: "a procedure's kept program is run after a definition it was bound with changed",
        file: "src/db/routines/kept.rs",
        original: ".filter(|(key, _)| key.holds(st, session))",
        replacement: ".filter(|(key, _)| key.holds(st, session) || true)",
        integration: &["kept_routines::a_kept_body_is_bound_again_as_definitions_change"],
        unit: &[],
    },
    Mutation {
        name: "oltp-kept-body-holding-instant",
        package: Package::Core,
        breaks: "a procedure's program bound with the instant of one CALL is kept for the next",
        file: "src/db/routines/kept.rs",
        original: "        if !held {",
        replacement: "        if !held || true {",
        integration: &["kept_routines::a_body_that_reads_the_instant_takes_each_calls"],
        unit: &[],
    },
];
