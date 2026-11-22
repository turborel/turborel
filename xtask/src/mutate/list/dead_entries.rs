//! The mutations of index entries taken back by the scan that meets them
//! (RULES.md R-TEST-9, D837): what tells a read its row is gone, and what
//! keeps an entry a snapshot may still read; and of a scan priced by a
//! sample of its zones (D838).

use super::{Mutation, Package};

/// The mutations of entries taken back by a scan, in the order they run.
pub const DEAD_ENTRIES: [Mutation; 3] = [
    Mutation {
        name: "dead-entries-gone-untold",
        package: Package::Core,
        breaks: "a read finding a row deleted never tells the deletion, so a scan takes back no entry and passes the same dead ones again",
        file: "src/engine/rows/scan.rs",
        original: "            near.gone = self.deletion_of(&near.bytes);\n",
        replacement: "            near.gone = None;\n",
        integration: &["dead_entries::a_scan_takes_back_the_entries_of_rows_gone_for_every_reader"],
        unit: &["engine::rows::scan::tests::a_read_of_a_deleted_row_tells_its_deletion"],
    },
    Mutation {
        name: "dead-entries-horizon-ignored",
        package: Package::Core,
        breaks: "an entry of a row deleted after an open snapshot is taken back, so that snapshot no longer finds the row by the index",
        file: "src/exec/entries/pruning.rs",
        original: "        if at > self.horizon {\n            return false;\n        }\n",
        replacement: "",
        integration: &["dead_entries::an_older_snapshot_keeps_the_entries_it_may_read"],
        unit: &["exec::entries::pruning::tests::only_an_entry_of_a_row_gone_before_every_snapshot_is_taken_back"],
    },
    Mutation {
        name: "dead-entries-sampled-share-zero",
        package: Package::Core,
        breaks: "a scan priced by a sample of its zones that holds no row asked for is priced at nothing, below the index that finds them",
        file: "src/engine/rows/zones.rs",
        original: "        let least = if step > 1 { 1.0_f64 / sampled } else { 0.0_f64 };\n",
        replacement: "        let least = 0.0_f64;\n",
        integration: &[],
        unit: &["engine::rows::zones::costs::a_sampled_share_is_never_less_than_one_zone_of_it"],
    },
];
