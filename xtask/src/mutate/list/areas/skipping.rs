//! The mutations of the column store's skipping (RULES.md R-TEST-9,
//! D510): the shadow bits a write and a group published set, and the
//! zones of the tree's rows a scan reads the delta by.

use super::super::{Mutation, Package};

/// The shadow bits' and the delta zones' unit tests.
const BITS: &str = "engine::rows::frozen::shadowed::shadow_bits_mark_writes_and_keep_scans_the_same";
const DELTA: &str = "engine::rows::frozen::shadowed::delta_zones_skip_the_tree_rows_no_condition_matches";

/// Mechanisms whose removal must fail the skipping's requirements.
pub const SKIPPING: [Mutation; 3] = [
    Mutation {
        name: "shadow-bits-writer",
        package: Package::Core,
        breaks: "a change of a frozen row sets no shadow bit, so a scan reads its group's old value",
        file: "src/engine/rows/mvcc/versions.rs",
        original: "        self.mark_shadow(id);\n        if thawed.is_some() {",
        replacement: "        if thawed.is_some() {",
        integration: &[],
        unit: &[BITS],
    },
    Mutation {
        name: "shadow-bits-published",
        package: Package::Core,
        breaks: "a group published marks no entry the tree holds in its range, so a row left open as it froze is missed",
        file: "src/engine/rows/frozen/freeze.rs",
        original: "            self.mark_published(m, &entries)?;",
        replacement: "            let _ = m;",
        integration: &[],
        unit: &[BITS],
    },
    Mutation {
        name: "delta-zones",
        package: Package::Core,
        breaks: "a scan reads every row of the tree, whatever zones its condition may match",
        file: "src/engine/rows/frozen/batches.rs",
        original: "RowTable::ranged(&self.table, self.table.zones.ranges(self.condition.as_ref()))",
        replacement: "RowTable::ranged(&self.table, vec![(0, None)])",
        integration: &[],
        unit: &[DELTA],
    },
];
