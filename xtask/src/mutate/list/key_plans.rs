//! The mutations of access paths no estimate can make a scan (RULES.md
//! R-TEST-9, D839), of a whole unique key probed whatever the table's
//! size, a key's range never priced above the scan of its conjuncts and
//! taken at the tie, a scan never priced below one zone, a plan kept
//! learned again once its tables have statistics, a join weighing its
//! probes by its left's index range (D890), a range past the
//! statistics estimated by its index (D891), a range of bounds a
//! constant apart by its width, and a variable named twice one slot of a
//! correlated subquery's plan (D892).

use super::{Mutation, Package};

/// The mutations of key plans, in the order they run.
pub const KEY_PLANS: [Mutation; 9] = [
    Mutation {
        name: "key-plans-point-sized",
        package: Package::Core,
        breaks: "a probe of a whole unique key is taken only of a table of a thousand rows, so a small table, or one empty as its plan is kept, is scanned",
        file: "src/physical/access.rs",
        original: "        let (storage, desc) = (self.db.storage_of(read.table).ok()?, self.db.catalog.table(read.table)?);\n",
        replacement: "        let (storage, desc) = (self.db.storage_of(read.table).ok()?, self.db.catalog.table(read.table)?);\n        if storage.row_count() < 1024 {\n            return None;\n        }\n",
        integration: &[
            "key_plans::a_whole_unique_key_is_probed_whatever_the_statistics",
            "key_plans::a_whole_key_kept_before_the_load_reads_one_row",
        ],
        unit: &[],
    },
    Mutation {
        name: "key-plans-range-unbounded",
        package: Package::Core,
        breaks: "a key's range is priced by its fetches alone, above the scan of its zones, so a stock level scans the newest orders' zones at each call",
        file: "src/physical/access/bounded.rs",
        original: "        cost.min(scan + rows * on.evaluation.per_row)\n",
        replacement: "        let _ = (scan, rows);\n        cost\n",
        integration: &["tpcc_plans::tpcc_procedures_read_by_index_under_any_statistics"],
        unit: &[],
    },
    Mutation {
        name: "key-plans-range-tie-to-scan",
        package: Package::Core,
        breaks: "a key's range priced as the scan of its conjuncts is never taken at the tie, so the scan reads the zones of every later execution",
        file: "src/physical/access/bounded.rs",
        original: "    path.bounded && path.total <= best.total && path.est < best.est && path.est * margin < all\n",
        replacement: "    path.bounded && path.total < best.total && path.est < best.est && path.est * margin < all\n",
        integration: &["tpcc_plans::tpcc_procedures_read_by_index_under_any_statistics"],
        unit: &["physical::access::bounded::tests::a_bounded_range_is_taken_at_a_tie_where_it_reads_fewer_rows"],
    },
    Mutation {
        name: "key-plans-statistics-unseen",
        package: Package::Core,
        breaks: "a plan kept notes every table as having had statistics, so one learned of the defaults is kept once they are gathered",
        file: "src/db/prepared/plans/grown.rs",
        original: "    reads.iter().map(|t| st.statistics(t).is_some()).collect()\n",
        replacement: "    reads.iter().map(|_| true).collect()\n",
        integration: &[
            "key_plans::a_scan_learned_before_statistics_is_learned_again_once_they_are",
            "key_plans::a_procedures_scan_learned_before_statistics_is_learned_again",
            "tpcc_plans::a_probes_plan_learned_before_statistics_is_learned_again_once_they_are",
        ],
        unit: &[],
    },
    Mutation {
        name: "key-plans-zone-floor-sampled-only",
        package: Package::Core,
        breaks: "a scan whose every zone is looked at, none holding a row asked for now, is priced at nothing, and a plan kept with it reads the rows written since",
        file: "src/engine/rows/zones.rs",
        original: "        (read_rows / all).max(1.0_f64 / sampled)\n",
        replacement: "        (read_rows / all).max(if step > 1 { 1.0_f64 / sampled } else { 0.0_f64 })\n",
        integration: &[],
        unit: &["engine::rows::zones::costs::a_sampled_share_is_never_less_than_one_zone_of_it"],
    },
    Mutation {
        name: "key-plans-range-rows-ignored",
        package: Package::Core,
        breaks: "a join weighs its probes by the statistics' estimate of its left alone, so a stock level's lines past the last pass, estimated at every line written since, hash a read of the whole of STOCK",
        file: "src/physical/indexed/ranged.rs",
        original: "    built.and_then(ranged).map_or(estimated, |rows| estimated.min(rows))\n",
        replacement: "    built.and_then(ranged).map_or(estimated, |rows| estimated.max(rows))\n",
        integration: &[],
        unit: &["physical::indexed::ranged::tests::a_left_reading_an_index_range_is_estimated_at_its_rows_if_fewer"],
    },
    Mutation {
        name: "key-plans-past-range-uncapped",
        package: Package::Core,
        breaks: "a range past the greatest value the statistics saw keeps every row written since, so a stock level's plan made with slots hashes a read of the whole of STOCK and is kept",
        file: "src/physical/cardinality/past.rs",
        original: "            least = least.min(cost_of(rows) / all.max(1.0_f64));\n",
        replacement: "            least = least.max(cost_of(rows) / all.max(1.0_f64));\n",
        integration: &[],
        unit: &["physical::cardinality::tests::a_range_past_the_statistics_is_estimated_by_its_index"],
    },
    Mutation {
        name: "key-plans-slot-width-uncapped",
        package: Package::Core,
        breaks: "a range between two bounds of one slot is weighed by the statistics alone with no values at hand, so a stock level's plan made with none hashes a read of the whole of STOCK",
        file: "src/physical/cardinality/width.rs",
        original: "            least = least.min(width * rows / all.max(1.0_f64));\n",
        replacement: "            least = least.max(width * rows / all.max(1.0_f64));\n",
        integration: &[],
        unit: &["physical::cardinality::tests::a_range_of_a_slot_is_estimated_by_its_width"],
    },
    Mutation {
        name: "key-plans-outer-slots-apart",
        package: Package::Core,
        breaks: "a variable named twice in a procedure's statement is two slots of its plan, so a range between it less twenty and it bounds no width and is weighed by the statistics alone",
        file: "src/exec/subquery/planned.rs",
        original: "    found.iter().enumerate().map(|(at, n)| n.and_then(|n| found.iter().position(|m| *m == Some(n))).unwrap_or(at)).collect()\n",
        replacement: "    found.iter().enumerate().map(|(at, _)| at).collect()\n",
        integration: &[],
        unit: &["exec::subquery::planned::tests::a_variable_named_twice_is_one_slot"],
    },
];
