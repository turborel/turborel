//! The mutations of late materialization (RULES.md R-TEST-9, D360, D361,
//! D366):
//! the rows a scan judges intersected with those the tree shadows, texts
//! coded by symbols compared padded with spaces, a value read alone of a
//! frame of reference, a comparison true of every value leaving out the
//! nulls, a late column's places composed through a join, and a judged
//! column's values held of the rows kept with their nulls, from the
//! batch's first run, and a morsel's columns told late given late (D366).

use super::super::{Mutation, Package};

/// Rows changed after they were frozen, read late.
const CHANGED: &str = "late_columns::late_columns_of_changed_rows_answer_as_rows";

/// A judged column held of the rows kept of several runs.
const HELD: &str = "engine::rows::frozen::batches::pending::tests::judged_columns_are_held_of_the_rows_kept";

/// Every query of the tables read late and not.
const ROWS: &str = "late_columns::late_columns_answer_as_rows";

/// Mechanisms whose removal must fail late materialization's requirements.
pub const LATE: [Mutation; 8] = [
    Mutation {
        name: "late-shadowed-rows-judged",
        package: Package::Core,
        breaks: "a scan judges only the rows of a run the tree has no entry for",
        file: "src/engine/rows/frozen/batches/run.rs",
        original: "        let given = positions.map(|p| p.iter().map(|&q| q.saturating_sub(from)).collect());",
        replacement: "        let given: Option<Vec<u32>> = positions.and(None);",
        integration: &[CHANGED],
        unit: &[],
    },
    Mutation {
        name: "late-symbols-unpadded",
        package: Package::Core,
        breaks: "a text coded by symbols compares with a constant padded with spaces",
        file: "src/engine/columns/fsst.rs",
        original: "            let k = rest.next().copied().unwrap_or(PAD);",
        replacement: "            let k = rest.next().copied().unwrap_or(0);",
        integration: &[],
        unit: &["engine::columns::judged::tests::texts_coded_by_symbols_compare_padded"],
    },
    Mutation {
        name: "late-frame-alone-unbased",
        package: Package::Core,
        breaks: "a value read alone of a frame of reference adds the frame's least",
        file: "src/engine/columns/encoded/words.rs",
        original: "            Body::Frame { least, bits, deltas } => get(deltas, *bits, n).map(|d| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()))),",
        replacement: "            Body::Frame { bits, deltas, .. } => get(deltas, *bits, n).map(|d| i64::from_le_bytes(d.to_le_bytes())),",
        integration: &[ROWS],
        unit: &[],
    },
    Mutation {
        name: "late-all-keeps-nulls",
        package: Package::Core,
        breaks: "a comparison true of every integer keeps none of the nulls",
        file: "src/engine/columns/judged.rs",
        original: "            Some(Bound::All) => out.extend(rows(ints.units.len(), given).into_iter().filter(|&r| !ints.is_null(index(r)))),",
        replacement: "            Some(Bound::All) => out.extend(rows(ints.units.len(), given)),",
        integration: &[],
        unit: &["engine::columns::judged::tests::numbers_keep_what_their_values_keep"],
    },
    Mutation {
        name: "late-picks-uncomposed",
        package: Package::Core,
        breaks: "a late column gathered twice composes the second picks with the first",
        file: "src/storage/batch/late.rs",
        original: "picks.iter().map(|p| p.and_then(|r| self.place(r)).and_then(|place| u32::try_from(place).ok())).collect();",
        replacement: "picks.iter().map(|p| p.and_then(|place| u32::try_from(place).ok())).collect();",
        integration: &[],
        unit: &["storage::batch::late::tests::late_columns_are_decoded_of_the_rows_kept"],
    },
    Mutation {
        name: "late-held-nulls-dropped",
        package: Package::Core,
        breaks: "a judged column's values held of the rows kept keep their nulls",
        file: "src/engine/rows/frozen/batches/pending.rs",
        original: "        mask.push(from.get(index(r)));",
        replacement: "        mask.push(false);",
        integration: &[ROWS],
        unit: &[HELD],
    },
    Mutation {
        name: "late-held-begun-late",
        package: Package::Core,
        breaks: "a judged column is held from the first run of a batch's rows, or not at all",
        file: "src/engine/rows/frozen/batches/pending.rs",
        original: "            } else if first {",
        replacement: "            } else if !first {",
        integration: &[],
        unit: &[HELD],
    },
    Mutation {
        name: "late-morsel-unlate",
        package: Package::Core,
        breaks: "a morsel of a column store gives the columns it is told late as their places",
        file: "src/engine/rows/split.rs",
        original: "what, self.zones_within(range)).with_late(late)))",
        replacement: "what, self.zones_within(range))))",
        integration: &[],
        unit: &["engine::rows::frozen::tests::morsels::morsels_give_late_columns_as_their_places"],
    },
];
