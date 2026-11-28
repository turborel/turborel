//! The mutations of late materialization (RULES.md R-TEST-9, D360, D361,
//! D366):
//! the rows a scan judges intersected with those the tree shadows, texts
//! coded by symbols compared padded with spaces, a value read alone of a
//! frame of reference, a comparison true of every value leaving out the
//! nulls, a late column's places composed through a join, and a judged
//! column's values held of the rows kept with their nulls, from the
//! batch's first run, a morsel's columns told late given late (D366), and
//! a join's keys asked by a scan before its other columns are decoded
//! (D1294).

use super::super::{Mutation, Package};

/// Rows changed after they were frozen, read late.
const CHANGED: &str = "late_columns::late_columns_of_changed_rows_answer_as_rows";

/// A judged column held of the rows kept of several runs.
const HELD: &str = "engine::rows::frozen::batches::pending::tests::judged_columns_are_held_of_the_rows_kept";

/// Every query of the tables read late and not.
const ROWS: &str = "late_columns::late_columns_answer_as_rows";

/// A join's keys asked by a scan: the keys alone, the scan's runs, the
/// filter that tells them, and a join's scans counted.
const KEYS_HOLD: &str = "storage::condition::members::tests::each_form_holds_its_keys";
const KEYS_STOP: &str = "storage::condition::members::tests::a_scan_stops_asking_once_its_first_runs_kept_too_many";
const KEYS_ROWS: &str = "engine::columns::judged::among::tests::a_joins_keys_keep_the_rows_whose_key_they_hold";
const KEYS_LEFT: &str = "engine::columns::judged::among::tests::a_scan_stops_asking_keys_that_keep_most_rows";
const KEYS_TOLD: &str = "exec::table::runtime::tests::the_keys_told_a_scan_hold_what_the_filter_keeps";
const KEYS_SCANNED: &str = "scan_keys::a_scan_told_few_keys_gives_their_rows_alone";

/// Mechanisms whose removal must fail late materialization's requirements.
pub const LATE: [Mutation; 15] = [
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
        original: "what, self.zones_within(range)).asked(given)))",
        replacement: "what, self.zones_within(range))))",
        integration: &[],
        unit: &["engine::rows::frozen::tests::morsels::morsels_give_late_columns_as_their_places"],
    },
    Mutation {
        name: "scan-keys-of-another-scale-asked",
        package: Package::Core,
        breaks: "an integer of another scale than the keys' is looked for among them as it is",
        file: "src/storage/condition/members.rs",
        original: "        if scale != self.scale {\n            return true;\n        }\n",
        replacement: "",
        integration: &[],
        unit: &[KEYS_HOLD],
    },
    Mutation {
        name: "scan-keys-asked-at-their-share",
        package: Package::Core,
        breaks: "a scan goes on asking keys that kept exactly the share of rows a run is given whole at",
        file: "src/storage/condition/members.rs",
        original: "self.kept.load(Ordering::Relaxed).saturating_mul(share) < self.asked.load(Ordering::Relaxed)",
        replacement: "self.kept.load(Ordering::Relaxed).saturating_mul(share) <= self.asked.load(Ordering::Relaxed)",
        integration: &[],
        unit: &[KEYS_STOP, KEYS_LEFT],
    },
    Mutation {
        name: "scan-keys-never-left",
        package: Package::Core,
        breaks: "a scan asks a join's keys of every run, whatever its first runs kept",
        file: "src/engine/columns/judged/proved.rs",
        original: "self.among().is_some_and(|members| !members.asking(KEPT_SHARE))",
        replacement: "self.among().is_some_and(|members| !members.asking(0))",
        integration: &[],
        unit: &[KEYS_LEFT],
    },
    Mutation {
        name: "scan-keys-runs-not-counted",
        package: Package::Core,
        breaks: "the rows a join's keys kept of a scan's first runs are not counted, so it stops asking keys that keep few",
        file: "src/engine/columns/judged.rs",
        original: "                pred.counted(asked, rows.len());",
        replacement: "                pred.counted(asked, asked);",
        integration: &[],
        unit: &[KEYS_LEFT],
    },
    Mutation {
        name: "scan-keys-keep-a-null",
        package: Package::Core,
        breaks: "a row whose key is null is kept by the unit at its place",
        file: "src/engine/columns/judged/among.rs",
        original: "ints.units.get(index(r)).copied().filter(|_| !ints.is_null(index(r)));",
        replacement: "ints.units.get(index(r)).copied();",
        integration: &[],
        unit: &[KEYS_ROWS],
    },
    Mutation {
        name: "scan-keys-bloom-of-any-scale",
        package: Package::Core,
        breaks: "a Bloom filter of keys not whole numbers is asked a hash the join did not make of them",
        file: "src/storage/condition/members.rs",
        original: "                if scale != 0 || u.unsigned_abs() >= MAGIC_UNITS {",
        replacement: "                if u.unsigned_abs() >= MAGIC_UNITS {",
        integration: &[],
        unit: &[KEYS_HOLD],
    },
    Mutation {
        name: "scan-keys-not-told",
        package: Package::Core,
        breaks: "a scan is told a filter's range and spans without its keys, and gives every row of each part they reach",
        file: "src/exec/table/runtime.rs",
        original: "            told.push(Conjunct::Among { column, members: Arc::clone(members) });",
        replacement: "",
        integration: &[KEYS_SCANNED],
        unit: &[KEYS_TOLD],
    },
];
