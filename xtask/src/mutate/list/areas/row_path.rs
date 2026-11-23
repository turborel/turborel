//! The mutations of the row path (RULES.md R-TEST-9, D750, D751): a row
//! change's log record made from the images as they are held, sealed from
//! its bytes; collection reading only the versions whose entries it takes
//! back, and of those only the columns their keys read; and a record
//! decoding each column asked for once.

use super::super::{Mutation, Package};

/// Mechanisms of the row path whose removal must fail their tests.
pub const ROW_PATH: [Mutation; 9] = [
    Mutation {
        name: "row-path-update-images-swapped",
        package: Package::Core,
        breaks: "an update's record made from held rows writes its after-image before its before-image",
        file: "src/log/record/batch.rs",
        original: "    super::images::put_update(out, (table, generation, id), before, after);",
        replacement: "    super::images::put_update(out, (table, generation, id), after, before);",
        integration: &[],
        unit: &["log::record::tests::records_made_from_held_rows_are_their_changes"],
    },
    Mutation {
        name: "row-path-row-change-unsealed",
        package: Package::Core,
        breaks: "a row change of a sealed table is logged in the clear",
        file: "src/txn/write.rs",
        original: "                tx.log_encoded(&seal_plain(&plain, id, &pipeline)?, 1)",
        replacement: "                tx.log_encoded(&plain, u64::from(seal_plain(&[], id, &pipeline).is_ok()))",
        integration: &["table_encodings::an_encrypted_tables_log_records_are_sealed"],
        unit: &[],
    },
    Mutation {
        name: "row-path-collection-reads-the-unneeded",
        package: Package::Core,
        breaks: "collection reads the versions it does not need and takes back no entry of those it does",
        file: "src/engine/rows/mvcc/gc.rs",
        original: "        let gone = if removed.needs(kept) {",
        replacement: "        let gone = if !removed.needs(kept) {",
        integration: &[],
        unit: &["engine::rows::tests::collection_reads_only_the_versions_it_needs"],
    },
    Mutation {
        name: "row-path-global-entries-unread",
        package: Package::Core,
        breaks: "collection reads no version of a partition under a global index whose own entries are shared, so its global entries stay",
        file: "src/txn/kept.rs",
        original: "    globals || indexes.into_iter().any(|sharing| !sharing.shares(kept))",
        replacement: "    indexes.into_iter().any(|sharing| !sharing.shares(kept))",
        integration: &[],
        unit: &["txn::kept::tests::a_version_is_read_only_for_entries_of_its_own"],
    },
    Mutation {
        name: "row-path-named-column-decoded-again",
        package: Package::Core,
        breaks: "a column named twice is decoded twice, its value out of line fetched twice",
        file: "src/engine/record.rs",
        original: "            if let Some(slot) = row.get_mut(i).filter(|slot| slot.is_null()) {",
        replacement: "            if let Some(slot) = row.get_mut(i) {",
        integration: &[],
        unit: &["engine::record::tests::columns_asked_for_are_decoded_once"],
    },
    Mutation {
        name: "row-path-collection-reads-every-column",
        package: Package::Core,
        breaks: "collection decodes the whole row of a version it reads, not the columns asked for",
        file: "src/engine/rows/mvcc/gc.rs",
        original: "            let row = self.decode(id, version.record, removed.columns())?;",
        replacement: "            let row = self.decode(id, version.record, None)?;",
        integration: &[],
        unit: &["engine::rows::tests::collection_reads_only_the_versions_it_needs"],
    },
    Mutation {
        name: "row-path-collection-reads-no-key",
        package: Package::Core,
        breaks: "collection reads none of the columns its entries' keys read, so it takes back no entry",
        file: "src/txn/kept.rs",
        original: "    let mut reads: Vec<usize> = indexes.flat_map(IndexDesc::reads).collect();",
        replacement: "    let mut reads: Vec<usize> = indexes.take(0).flat_map(IndexDesc::reads).collect();",
        integration: &["kept_entries::a_key_changed_and_back_is_found_by_each_snapshot"],
        unit: &[],
    },
    Mutation {
        name: "row-path-listed-unordered",
        package: Package::Core,
        breaks: "a read of listed columns gives a column the record lacks no value, so the values after it are out of their places",
        file: "src/engine/record/listed.rs",
        original: "let value = if i < self.types.len() { self.value_in(bytes, i, fetch) } else { Ok(Value::Null) };",
        replacement: "let value = if i < self.types.len() { self.value_in(bytes, i, fetch) } else { continue };",
        integration: &[],
        unit: &["engine::record::tests::listed_columns_are_appended_in_their_order"],
    },
    Mutation {
        name: "row-path-count-one-way",
        package: Package::Core,
        breaks: "a table's count of rows only rises, a deletion counted as a row more",
        file: "src/engine/rows/counted.rs",
        original: "stripe.0.fetch_add(if up { by } else { by.saturating_neg() }, Ordering::Relaxed);",
        replacement: "stripe.0.fetch_add(by, Ordering::Relaxed);",
        integration: &[],
        unit: &["engine::rows::counted::tests::a_count_moved_by_threads_at_once_sums_exactly"],
    },
];
