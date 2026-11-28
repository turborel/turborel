//! The mutations of texts a scan does not make because nobody reads them
//! (RULES.md R-TEST-9): a dictionary chunk's entries made text on first
//! use and of character strings alone (D1298); and a column given
//! measured, its lengths of its codes, its entries and its nulls, of
//! VARCHAR alone, left late at the stream's end, gathered with its
//! nulls, asked for where no filter reads its text, and the one
//! comparison its lengths tell, in the filter and in the scan (D1267).

use super::super::{Mutation, Package};

/// The file of the chunks a scan opens once.
const PREPARED: &str = "src/engine/columns/prepared.rs";

/// The test of when a dictionary's entries are made text.
const FIRST_ASKED: &str = "engine::columns::prepared::tests::a_dictionarys_entries_are_made_text_when_first_asked_for";
/// The test of entries that are no text.
const NOT_TEXT: &str = "engine::columns::prepared::tests::entries_not_text_give_none_however_late_they_are_made";

/// The test of a measured column's comparison judged of its lengths.
const BY_LENGTHS: &str =
    "engine::columns::judged::lengths::tests::a_measured_columns_comparison_with_the_empty_string_is_judged_of_its_lengths";

/// Mechanisms whose removal must fail the requirements of texts made
/// only for a reader.
pub const UNREAD_TEXTS: [Mutation; 13] = [
    Mutation {
        name: "entries-made-text-at-open",
        package: Package::Core,
        breaks: "a dictionary chunk's entries are made text as the chunk is opened, read or not",
        file: PREPARED,
        original: "        let made = Made { words, ..Made::default() };",
        replacement: "        let made = Made { words, ..Made::default() };\n        made.texts_of(&chunk, (t, pages.kernels));",
        integration: &[],
        unit: &[FIRST_ASKED],
    },
    Mutation {
        name: "entries-made-text-of-any-type",
        package: Package::Core,
        breaks: "a dictionary's entries are made text for a column that holds no character strings",
        file: PREPARED,
        original: "            padding(t).and_then(|pad| chunk.body.entry_texts(pad, kernels))",
        replacement: "            chunk.body.entry_texts(padding(t).flatten(), kernels)",
        integration: &[],
        unit: &[NOT_TEXT],
    },
    Mutation {
        name: "measured-escape-counted-twice",
        package: Package::Core,
        breaks: "an escaped octet of a text coded by symbols is counted as two",
        file: "src/engine/columns/fsst/lengths.rs",
        original: "                octets = octets.saturating_add(1);",
        replacement: "                octets = octets.saturating_add(2);",
        integration: &[],
        unit: &["engine::columns::fsst::lengths::tests::a_texts_codes_count_the_octets_it_decodes_to"],
    },
    Mutation {
        name: "measured-null-as-its-place-holds",
        package: Package::Core,
        breaks: "a null's length is what its place in the chunk holds, as a constant chunk's one value",
        file: "src/engine/columns/encoded/lengths.rs",
        original: "            if null {",
        replacement: "            if null && places.is_empty() {",
        integration: &[],
        unit: &["engine::columns::encoded::lengths::tests::each_encoding_gives_its_values_octet_lengths"],
    },
    Mutation {
        name: "measured-row-by-its-place",
        package: Package::Core,
        breaks: "a row of a dictionary is given the length of the entry of its place, not of its code",
        file: "src/engine/columns/encoded/lengths.rs",
        original: "            let entry = usize::try_from(*held.get(n.checked_sub(low)?)?).ok()?;",
        replacement: "            let entry = n.checked_rem(count)?;",
        integration: &[],
        unit: &["engine::columns::encoded::lengths::tests::each_encoding_gives_its_values_octet_lengths"],
    },
    Mutation {
        name: "measured-of-a-padded-column",
        package: Package::Core,
        breaks: "a CHARACTER column is measured by the octets written, though its values are padded as read",
        file: "src/engine/columns/scan/lengths.rs",
        original: "    if padding(t) != Some(None) {",
        replacement: "    if padding(t).is_none() {",
        integration: &["measured_columns::measured_columns_answer_as_rows"],
        unit: &[],
    },
    Mutation {
        name: "measured-decoded-at-the-end",
        package: Package::Core,
        breaks: "a measured column is decoded as the stream is read whole, though no reader takes its text",
        file: "src/storage/batch/late.rs",
        original: "                && !late.is_measured()",
        replacement: "                && late.len() < usize::MAX",
        integration: &[],
        unit: &["storage::batch::late::tests::a_measured_late_column_gives_its_lengths_and_is_left_late"],
    },
    Mutation {
        name: "measured-null-gathered-as-zero",
        package: Package::Core,
        breaks: "a null gathered by a join's picks is given the length 0, not a null",
        file: "src/storage/batch/late/measured.rs",
        original: "filter(|&p| p < held.units.len() && !held.is_null(p));",
        replacement: "filter(|&p| p < held.units.len());",
        integration: &[],
        unit: &["storage::batch::late::tests::a_measured_late_column_gives_its_lengths_and_is_left_late"],
    },
    Mutation {
        name: "measured-any-constant-by-length",
        package: Package::Core,
        breaks: "a comparison unpadded with any constant is judged as one with the empty string",
        file: "src/exec/batch/judge/measured.rs",
        original: "    if !octets.is_empty() {",
        replacement: "    if octets.len() > usize::MAX {",
        integration: &[],
        unit: &["exec::batch::judge::measured::tests::the_empty_string_is_compared_by_lengths_alone"],
    },
    Mutation {
        name: "measured-under-a-filter-that-reads",
        package: Package::Core,
        breaks: "a column a filter reads the text of is still asked of its scan measured",
        file: "src/exec/batch/late.rs",
        original: "        let measured = without(&self.measured, &read_as_text(&[], &[cond]));",
        replacement: "        let measured = self.measured.clone();",
        integration: &[],
        unit: &["exec::batch::late::tests::a_column_read_for_its_length_alone_is_asked_measured"],
    },
    Mutation {
        name: "measured-columns-not-given",
        package: Package::Core,
        breaks: "a morsel asked for a column measured gives it decoded",
        file: "src/engine/rows/frozen/batches.rs",
        original: "        asked.measured = measured.iter().copied().filter(|c| asked.columns.contains(c)).collect();",
        replacement: "        asked.measured = measured.iter().copied().filter(|c| !asked.columns.contains(c)).collect();",
        integration: &[],
        unit: &["engine::rows::frozen::tests::morsels::a_morsel_gives_a_column_asked_measured_late_with_its_lengths"],
    },
    Mutation {
        name: "measured-judged-of-any-constant",
        package: Package::Core,
        breaks: "the scan judges a comparison with any constant of a measured column's lengths, as one with the empty string",
        file: "src/engine/columns/judged/lengths.rs",
        original: "    if !octets.is_empty() || !prepared.measures(pred.column) {",
        replacement: "    if !prepared.measures(pred.column) {",
        integration: &[],
        unit: &[BY_LENGTHS],
    },
    Mutation {
        name: "measured-lengths-of-another-column",
        package: Package::Core,
        breaks: "the lengths held of one column are given as another's",
        file: "src/engine/columns/prepared/lengths.rs",
        original: "        if *held != column || !whole",
        replacement: "        if !whole",
        integration: &[],
        unit: &[BY_LENGTHS],
    },
];
