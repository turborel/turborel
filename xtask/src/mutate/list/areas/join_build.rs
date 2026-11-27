//! The mutations of a hash join's operand built by the threads that read
//! it (RULES.md R-TEST-9, D1080): a run's rows sorted by part in the
//! order built, a part made of the runs in order, a direct table's rows
//! of a null key kept, keys given as numbers hashed for a hashed table,
//! each vector of the batch joined of its own column, the runtime
//! filters made beside the table, and an operand of a morsel's rows left
//! to the statement's thread; of D1081, the probing table's unread
//! columns carried late across a pipeline's probe; of D1082, a grouping
//! by an expression bounded by the columns it reads, and only by a
//! deterministic one; of D1083, a null probing row paired with none, and
//! keys of scales apart or of matching nulls paired as values, and a
//! row's matches in the order built; of D1084, integers known as runs
//! judged of the rows asked of alone, a run no row asks of left unjudged,
//! and a null of a run kept never; of D1086, a join on keys computed of
//! each operand's columns, and only of deterministic ones; of D1087, a
//! probe of a table past a thread's share of the cache screened by its
//! Bloom filter on the kernels, and a semi join of many held keys
//! filtering its right.

use super::super::{Mutation, Package};

/// The parted table's file.
const PARTED: &str = "src/exec/table/join/parted.rs";

/// The file of the build by many threads.
const BUILD: &str = "src/exec/parallel/build.rs";

/// The test of a hashed table made in parts.
const HASHED: &str = "exec::table::join::parted::tests::a_hashed_table_in_parts_finds_what_one_thread_s_finds";

/// The test of a table built by many threads.
const BUILT: &str = "exec::parallel::build::tests::a_table_built_by_many_threads_is_the_one_a_thread_builds";

/// Mechanisms whose removal must fail the requirements of a join's
/// operand built by many threads.
/// The file of a key of integers paired in its units.
const PAIRED: &str = "src/exec/batch/joined/shared/paired.rs";

/// The test of keys that are not paired in their units.
const AS_VALUES: &str = "exec::batch::joined::shared::paired::tests::keys_of_scales_apart_or_matching_nulls_pair_as_values";

/// The file of integers judged by runs.
const RUNS: &str = "src/storage/batch/runs.rs";

/// The test of rows kept by runs.
const BY_RUNS: &str = "storage::batch::runs::tests::rows_kept_by_runs_are_the_rows_judged_one_at_a_time";

/// The file of a join on computed keys.
const COLLATED: &str = "src/physical/collated.rs";

/// The test of joins on computed keys.
const COMPUTED: &str = "computed_keys::an_equality_of_computed_keys_is_a_hash_join";

pub const JOIN_BUILD: [Mutation; 30] = [
    Mutation {
        name: "probe-units-reversed",
        package: Package::Core,
        breaks: "a key paired in its units gives a row's matches in the order they were built (D1083)",
        file: "src/exec/batch/joined/shared/probing.rs",
        original: "            for &p in table.table.rows_of(key).get(self.seen..).unwrap_or_default() {",
        replacement: "            for &p in table.table.rows_of(key).get(self.seen..).unwrap_or_default().iter().rev() {",
        integration: &[],
        unit: &["exec::batch::joined::shared::paired::tests::a_key_of_integers_paired_in_its_units_pairs_as_values_do"],
    },
    Mutation {
        name: "probe-units-null-pairs",
        package: Package::Core,
        breaks: "a null probing row of a key paired in its units pairs with no row (D1083)",
        file: "src/exec/batch/joined/shared/probing.rs",
        original: "probe.units.get(r).filter(|_| !probe.is_null(r)) else { return Ok(true) };",
        replacement: "probe.units.get(r) else { return Ok(true) };",
        integration: &[],
        unit: &[AS_VALUES],
    },
    Mutation {
        name: "probe-units-scales-apart",
        package: Package::Core,
        breaks: "keys of integers of scales apart are compared as values, not by their units (D1083)",
        file: PAIRED,
        original: "        (probe.scale == built.scale).then_some((probe, built))",
        replacement: "        Some((probe, built))",
        integration: &[],
        unit: &[AS_VALUES],
    },
    Mutation {
        name: "probe-units-nulls-match",
        package: Package::Core,
        breaks: "a key whose nulls match is paired as values, a null with a null (D1083)",
        file: PAIRED,
        original: "let ([l], [at], [false]) =",
        replacement: "let ([l], [at], [_]) =",
        integration: &[],
        unit: &[AS_VALUES],
    },
    Mutation {
        name: "estimate-expression-unbounded",
        package: Package::Core,
        breaks: "a grouping counts of an expression at most the combinations of the columns it reads (D1082)",
        file: "src/physical/cardinality/nodes.rs",
        original: "        computed.read.iter().map(of).product::<f64>().min(whole)",
        replacement: "        computed.read.iter().map(of).product::<f64>().max(whole)",
        integration: &[],
        unit: &["physical::cardinality::tests::a_grouping_by_an_expression_is_bounded_by_its_columns"],
    },
    Mutation {
        name: "estimate-expression-drawn",
        package: Package::Core,
        breaks: "an expression that is not deterministic is bounded by no column (D1082)",
        file: "src/physical/cardinality/lineage.rs",
        original: "            return expr_pure(expr).then_some(Computed { input, expr, read });",
        replacement: "            return Some(Computed { input, expr, read });",
        integration: &[],
        unit: &["physical::cardinality::tests::an_expression_not_deterministic_is_not_bounded"],
    },
    Mutation {
        name: "join-probe-unlate",
        package: Package::Core,
        breaks: "a pipeline's probe carries the probing table's unread columns late (D1081)",
        file: "src/exec/parallel/batched.rs",
        original: "            Step::Probe(table) => deferred.left_of(table.keys(), left),",
        replacement: "            Step::Probe(_) => Deferred::default(),",
        integration: &[],
        unit: &["exec::parallel::batched::tests::a_probe_carries_the_probing_tables_unread_columns_late"],
    },
    Mutation {
        name: "join-build-run-order",
        package: Package::Core,
        breaks: "a run's rows of a part stay in the order they were built",
        file: PARTED,
        original: "        for (part, pair) in keyed {",
        replacement: "        for (part, pair) in keyed.into_iter().rev() {",
        integration: &[],
        unit: &[HASHED],
    },
    Mutation {
        name: "join-build-runs-in-order",
        package: Package::Core,
        breaks: "a part is made of every run's rows, the runs in order",
        file: PARTED,
        original: "        let built = || runs.iter().flat_map(",
        replacement: "        let built = || runs.iter().rev().flat_map(",
        integration: &[],
        unit: &[HASHED],
    },
    Mutation {
        name: "join-build-null-rows",
        package: Package::Core,
        breaks: "a direct table made in parts keeps the rows of a null key whose nulls match",
        file: PARTED,
        original: "                nulls.push(row);",
        replacement: "                nulls.clear();",
        integration: &[],
        unit: &["exec::table::join::parted::tests::a_direct_table_in_parts_finds_what_one_thread_s_finds"],
    },
    Mutation {
        name: "join-build-numbers-unhashed",
        package: Package::Core,
        breaks: "keys asked as numbers are hashed for a table that is not direct",
        file: BUILD,
        original: "        if numbers && !parting.direct() {",
        replacement: "        if numbers && parting.direct() {",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-one-column",
        package: Package::Core,
        breaks: "each vector of the built batch is joined of the parts' vectors at its place",
        file: "src/storage/batch/held.rs",
        original: "            parts.iter().map(|part| (part.columns.get(at), all.get(..part.rows).unwrap_or_default())).collect();",
        replacement: "            parts.iter().map(|part| (part.columns.get(at.min(1)), all.get(..part.rows).unwrap_or_default())).collect();",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-no-filters",
        package: Package::Core,
        breaks: "a table built by many threads makes the runtime filters of its keys",
        file: BUILD,
        original: "            Some(k) => Ok(Made::Filter(held.filter(k)?.map(Arc::new))),",
        replacement: "            Some(_) => Ok(Made::Filter(None)),",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-small-alone",
        package: Package::Core,
        breaks: "an operand of no more rows than a morsel is built by the statement's thread",
        file: BUILD,
        original: "    if crew.degree < 2 || rows <= crew.run_rows() {",
        replacement: "    if crew.degree < 2 || rows < crew.run_rows() {",
        integration: &[],
        unit: &["exec::parallel::build::tests::a_small_operand_or_one_thread_builds_alone"],
    },
    Mutation {
        name: "runs-unasked-judged",
        package: Package::Core,
        breaks: "a run that holds no row asked of is not judged (D1084)",
        file: RUNS,
        original: "            let asked = rest.is_none() || !these.is_empty();",
        replacement: "            let asked = true;",
        integration: &[],
        unit: &[BY_RUNS],
    },
    Mutation {
        name: "runs-every-place",
        package: Package::Core,
        breaks: "a run kept gives the rows asked of alone, not every place of it (D1084)",
        file: RUNS,
        original: "                self.push_run((start, end), given.map(|_| these), &mut out);",
        replacement: "                self.push_run((start, end), None, &mut out);",
        integration: &[],
        unit: &[BY_RUNS],
    },
    Mutation {
        name: "runs-null-kept",
        package: Package::Core,
        breaks: "a null of a run kept is not kept (D1084)",
        file: RUNS,
        original: "            (None, Some(nulls)) => out.extend(places().filter(|&r| !nulls.get(index(r)))),",
        replacement: "            (None, Some(_)) => out.extend(places()),",
        integration: &[],
        unit: &[BY_RUNS],
    },
    Mutation {
        name: "computed-keys-unlifted",
        package: Package::Core,
        breaks: "an equality of an expression of each operand's columns is a hash join on the computed keys (D1086)",
        file: COLLATED,
        original: "    match (of_left(first, width)?, of_left(second, width)?) {",
        replacement: "    match (of_left(first, width)?, of_left(first, width)?) {",
        integration: &[COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-impure",
        package: Package::Core,
        breaks: "a key of an expression that is not deterministic is not computed once a row (D1086)",
        file: COLLATED,
        original: "    if !e.all_subqueries().is_empty() || !expr_pure(e) {",
        replacement: "    if !e.all_subqueries().is_empty() {",
        integration: &[],
        unit: &["physical::collated::tests::keys_computed_of_each_operands_columns_are_equated"],
    },
    Mutation {
        name: "computed-keys-outer-raising",
        package: Package::Core,
        breaks: "an outer join's condition that can raise has no key computed under the join (D1086)",
        file: "src/physical/join.rs",
        original: "        let raises = cond.can_raise() && self.cond_raises(cond, (left, right));",
        replacement: "        let raises = false;",
        integration: &[COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "probe-screen-never",
        package: Package::Core,
        breaks: "a probe of a table past a thread's share of the cache is screened by its Bloom filter (D1087)",
        file: "src/exec/batch/joined/shared.rs",
        original: "|| self.table.bytes() <= SCREENED_BYTES {",
        replacement: "|| self.table.bytes() > 0 {",
        integration: &[],
        unit: &["exec::parallel::build::tests::probes_screened_by_the_bloom_filter_pair_as_every_probe"],
    },
    Mutation {
        name: "marked-filter-few-keys",
        package: Package::Core,
        breaks: "a semi join of more held keys than a filter lists filters its right (D1087)",
        file: "src/exec/batch/joined/semi/marked.rs",
        original: "        if !runtime::wanted(db) || in_order(read) {",
        replacement: "        if !runtime::wanted(db) || in_order(read) || self.batch.rows > 4_096 {",
        integration: &["marked_semi::a_built_left_of_many_keys_filters_its_right"],
        unit: &[],
    },
    Mutation {
        name: "simd-bloom-keeps-all",
        package: Package::Simd,
        breaks: "the vector Bloom probe keeps only the hashes the filter may hold (D1087)",
        file: "simd/src/x86/bloom.rs",
        original: "    out.truncate(len.saturating_add(keep));",
        replacement: "    out.truncate(len.saturating_add(keep | 1));",
        integration: &[],
        unit: &["tests::every_available_set_gives_the_portable_results"],
    },
    Mutation {
        name: "integers-hashed-unbounded",
        package: Package::Core,
        breaks: "a vector of integers of scale 0 is hashed by MAGIC only if each is below 2^51 in magnitude (D1098)",
        file: "src/exec/table/join/keys.rs",
        original: "            wide |= v.unsigned_abs() >= MAGIC_UNITS;\n",
        replacement: "",
        integration: &[],
        unit: &["exec::table::join::tests::integers_of_scale_zero_hash_as_their_buckets"],
    },
    Mutation {
        name: "integers-hashed-nulls-unmarked",
        package: Package::Core,
        breaks: "a null of a vector of integers hashed by MAGIC is a null's hash, a miss unless nulls match (D1098)",
        file: "src/exec/table/join/keys.rs",
        original: "            *h = how.null();\n",
        replacement: "",
        integration: &[],
        unit: &["exec::table::join::tests::integers_of_scale_zero_hash_as_their_buckets"],
    },
    Mutation {
        name: "built-parts-codes-decoded",
        package: Package::Core,
        breaks: "parts of one dictionary are put together as their codes (D1099)",
        file: "src/storage/batch/joined.rs",
        original: "    if let Some(codes) = super::coded::of_codes(parts, rows, || nulls(parts)) {",
        replacement: "    if let Some(codes) = None::<Column> {",
        integration: &[],
        unit: &["storage::batch::coded::tests::parts_of_one_dictionary_keep_their_codes"],
    },
    Mutation {
        name: "coded-gather-nulls-unmarked",
        package: Package::Core,
        breaks: "a pick of none, past the end or of a null gathered as a code is null (D1099)",
        file: "src/storage/batch/coded.rs",
        original: "                if none {\n                    null.set(n);\n                }\n",
        replacement: "",
        integration: &[],
        unit: &["storage::batch::coded::tests::small_built_texts_are_gathered_as_codes"],
    },
    Mutation {
        name: "dictionary-unbounded",
        package: Package::Core,
        breaks: "only a built vector of texts of no more rows than a batch's is made a dictionary (D1099)",
        file: "src/storage/batch/coded.rs",
        original: "t.len() <= MOST_ENTRIES;",
        replacement: "t.len() <= usize::MAX;",
        integration: &[],
        unit: &["storage::batch::coded::tests::small_built_texts_are_gathered_as_codes"],
    },
    Mutation {
        name: "pass-kept-unfiltered",
        package: Package::Core,
        breaks: "a probe in passes pairs the rows a pass keeps alone, and no other (D1242)",
        file: "src/exec/batch/joined/shared/kept.rs",
        original: "        retained(given, &keep);\n        retained(&mut hashes.hashes, &keep);\n        retained(&mut hashes.misses, &keep);\n",
        replacement: "",
        integration: &[],
        unit: &["exec::batch::joined::shared::kept::tests::a_probe_in_passes_pairs_the_rows_kept_alone"],
    },
    Mutation {
        name: "pass-kept-direct-numbers",
        package: Package::Core,
        breaks: "a probe of a direct table in passes is told each row's seeded hash, not its key's number (D1242)",
        file: "src/exec/batch/joined/shared/kept.rs",
        original: "        let keep: Vec<bool> = if self.table.is_direct() {",
        replacement: "        let keep: Vec<bool> = if false {",
        integration: &[],
        unit: &["exec::batch::joined::shared::kept::tests::a_probe_in_passes_pairs_the_rows_kept_alone"],
    },
];
