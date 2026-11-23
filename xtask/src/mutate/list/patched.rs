//! The mutations of a point UPDATE written as a patch of its row's record
//! (RULES.md R-TEST-9, D915 to D918): a column made null left its bytes,
//! a record with a value out of line patched, a record written whole of
//! the values set alone, a zone not widened, an engine that patches
//! nothing given the values set alone, a table sealed since patched, a
//! logged string without its spaces, a log record without the values
//! before, a write not told the columns it sets, a row read whole for
//! some columns, a row read without the columns its SET values read, a
//! row checked by a column it does not hold, and a CASE read by none of
//! its columns or by those of its later results alone.

use super::{Mutation, Package};

/// The file of a record made of another's bytes.
const RECORD: &str = "src/engine/record/patched.rs";
/// The file of the row store's patch of a version.
const PATCH: &str = "src/engine/rows/mvcc/patch.rs";
/// The file of the columns a select list reads at one execution.
const READ: &str = "src/db/routines/program/compiled/change/read.rs";
/// The unit test of a record patched in one column.
const ONE_COLUMN: &str = "engine::record::patched::tests::a_record_patched_in_one_column_is_the_record_encoded";
/// The unit test of the records that are not patched.
const REFUSED: &str = "engine::record::patched::tests::a_record_that_cannot_be_patched_is_refused";
/// The unit test of a record with a value out of line.
const OUT_OF_LINE: &str = "engine::rows::mvcc::patch::tests::a_record_with_a_value_out_of_line_is_written_whole";
/// The differential test of patches against whole writes.
const ALIKE: &str = "patched_updates::updates_written_as_patches_leave_what_whole_updates_leave";

/// The mutations of updates written as patches.
pub const PATCHED: [Mutation; 14] = [
    Mutation {
        name: "patch-null-zeroed",
        package: Package::Core,
        breaks: "a fixed column made null keeps the bytes of the value it had",
        file: RECORD,
        original: "            field.fill(0);\n",
        replacement: "",
        integration: &[],
        unit: &[ONE_COLUMN],
    },
    Mutation {
        name: "patch-outside-refused",
        package: Package::Core,
        breaks: "a record that keeps a value out of line is patched, its reference copied",
        file: RECORD,
        original: "        if end & OUTSIDE != 0 {\n            return None;\n        }\n        usize::try_from(end).ok()",
        replacement: "        usize::try_from(end & !OUTSIDE).ok()",
        integration: &[],
        unit: &[REFUSED],
    },
    Mutation {
        name: "patch-fallback-whole",
        package: Package::Core,
        breaks: "a record not patched is written of the values set alone, every other column null",
        file: PATCH,
        original: "let whole = patch.whole(self, tx, id, row)?;",
        replacement: "let whole = if patch.set.is_empty() { patch.whole(self, tx, id, row)? } else { row.clone() };",
        integration: &[],
        unit: &[OUT_OF_LINE],
    },
    Mutation {
        name: "patch-zone-widened",
        package: Package::Core,
        breaks: "a patch leaves its row's zone as it was",
        file: PATCH,
        original: "let widened = self.widen_patched(id, record, &replaced);",
        replacement: "let widened: Result<()> = Ok(());",
        integration: &[],
        unit: &["engine::rows::mvcc::patch::tests::a_patch_widens_its_rows_zone"],
    },
    Mutation {
        name: "patch-sealed-since",
        package: Package::Core,
        breaks: "a row of a table sealed after it was written is patched, and stored in the clear",
        file: PATCH,
        original: "self.sealing.passes_none() && self.lobs.columns().is_empty()",
        replacement: "self.lobs.columns().is_empty()",
        integration: &["patched_updates::a_row_of_a_table_sealed_since_is_written_sealed"],
        unit: &[],
    },
    Mutation {
        name: "patch-default-whole",
        package: Package::Core,
        breaks: "an engine that patches nothing writes the values set alone, every other column null",
        file: "src/storage/rows.rs",
        original: "    storage.replace(tx, id, Some(&patch.whole(storage, tx, id, new)?))",
        replacement: "    let _ = patch.whole(storage, tx, id, new)?;\n    storage.replace(tx, id, Some(new))",
        integration: &["patched_updates::an_engine_that_patches_nothing_writes_the_row_whole"],
        unit: &[],
    },
    Mutation {
        name: "patch-image-spaces",
        package: Package::Core,
        breaks: "a CHARACTER string of a patched row is logged without the spaces its record leaves out",
        file: "src/engine/record/image.rs",
        original: "Some(pad) => put_spaced(out, bytes, pad),",
        replacement: "Some(_) => put_spaced(out, bytes, 0),",
        integration: &[],
        unit: &["engine::record::image::tests::a_records_image_is_its_rows"],
    },
    Mutation {
        name: "patch-image-before",
        package: Package::Core,
        breaks: "a patched row's log record holds none of the values its columns had before",
        file: "src/log/record/images.rs",
        original: "spans.extend(set.iter().map(|(at, span)|",
        replacement: "spans.extend(set.iter().take(0).map(|(at, span)|",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "patch-columns-told",
        package: Package::Core,
        breaks: "a write whose row holds the columns set alone is not told them, and writes the row as it is",
        file: "src/txn/write.rs",
        original: "sparse.filter(|_| how == How::Patch)",
        replacement: "sparse.filter(|_| how != How::Patch)",
        integration: &[],
        unit: &["txn::writing::tests::an_update_holding_the_columns_set_alone_keeps_every_other"],
    },
    Mutation {
        name: "patch-entry-columns",
        package: Package::Core,
        breaks: "a point read asked for some columns of its row reads it whole",
        file: "src/exec/point/keyed.rs",
        original: "(None, Asked::Of(columns)) => probing.fetch(&entry, Some(columns))?,",
        replacement: "(None, Asked::Of(_)) => probing.fetch(&entry, None)?,",
        integration: &[],
        unit: &["exec::point::tests::an_entry_is_read_by_the_columns_asked_for"],
    },
    Mutation {
        name: "patch-reads-inputs",
        package: Package::Core,
        breaks: "a point UPDATE written as a patch reads its row without the columns its SET values read",
        file: "src/db/write/direct.rs",
        original: "Columns::of(inputs | keys | read)",
        replacement: "Columns::of(keys | read | (inputs & 0))",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "patch-checks-read",
        package: Package::Core,
        breaks: "an UPDATE whose CHECK reads a column it does not hold is written as a patch",
        file: "src/db/integrity/straight.rs",
        original: "let lean = set.is_some() && !straight.reads && bound.generated.is_empty();",
        replacement: "let lean = set.is_some() && bound.generated.is_empty();",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "patch-case-result",
        package: Package::Core,
        breaks: "a CASE judged ahead of its row reads none of the columns of the result it gives",
        file: READ,
        original: "Some(true) => return read_of(result, frame, deeper),",
        replacement: "Some(true) => return read_of(result, frame, 0) & 0,",
        integration: &[],
        unit: &["db::routines::program::compiled::change::read::tests::a_case_over_the_frame_reads_the_columns_of_its_result"],
    },
    Mutation {
        name: "patch-case-unjudged",
        package: Package::Core,
        breaks: "a CASE with a condition not judged ahead of its row reads the columns of its later results alone",
        file: READ,
        original: "            None => return all_of(e),\n",
        replacement: "            None => {}\n",
        integration: &[],
        unit: &["db::routines::program::compiled::change::read::tests::a_case_not_judged_ahead_reads_all_its_columns"],
    },
];
