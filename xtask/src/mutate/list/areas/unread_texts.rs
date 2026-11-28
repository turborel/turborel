//! The mutations of texts a scan does not make because nobody reads them
//! (RULES.md R-TEST-9): a dictionary chunk's entries made text on first
//! use and of character strings alone (D1298); and a column given
//! measured, its lengths of its codes, its entries and its nulls, of
//! VARCHAR alone, left late at the stream's end, gathered with its
//! nulls, asked for where no filter reads its text, and the one
//! comparison its lengths tell, in the filter and in the scan (D1267);
//! and a lengths lane, written of VARCHAR alone and of no hybrid group,
//! each length a record's, read back with its column, its pages the
//! group's, and the comparison with the empty string judged, proved and
//! passed over by it (D1319).

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
pub const UNREAD_TEXTS: [Mutation; 23] = [
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
    Mutation {
        name: "lane-of-a-padded-column",
        package: Package::Core,
        breaks: "a CHARACTER column is written a lengths lane, though its values are padded as read",
        file: "src/engine/columns/group/lanes.rs",
        original: "        if padding(t) != Some(None) {\n            lanes.push(None);",
        replacement: "        if padding(t).is_none() {\n            lanes.push(None);",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_group_with_no_lane_is_read_as_before"],
    },
    Mutation {
        name: "lane-length-of-the-end",
        package: Package::Core,
        breaks: "a lane holds where each record ends, not how long it is",
        file: "src/engine/columns/group/lanes.rs",
        original: "i64::from(end.saturating_sub(before))",
        replacement: "i64::from(end)",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_varchar_column_has_a_lane_of_its_lengths"],
    },
    Mutation {
        name: "lane-in-the-hybrid-layout",
        package: Package::Core,
        breaks: "a group in the hybrid layout is written lanes",
        file: "src/engine/columns/group/written.rs",
        original: "let lanes = if pages.pax {",
        replacement: "let lanes = if pages.pax && types.is_empty() {",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_group_with_no_lane_is_read_as_before"],
    },
    Mutation {
        name: "lane-section-not-read",
        package: Package::Core,
        breaks: "a header's lanes are not read, and its bytes past the sketches refused",
        file: "src/engine/columns/group/header.rs",
        original: "        if self.0.is_empty() {\n            return Ok(lanes);",
        replacement: "        if !self.0.is_empty() {\n            return Ok(lanes);",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_varchar_column_has_a_lane_of_its_lengths"],
    },
    Mutation {
        name: "lane-read-of-a-column-not-wanted",
        package: Package::Core,
        breaks: "a lane is read with a header read for other columns",
        file: "src/engine/columns/group/header.rs",
        original: "            if wanted.is_none_or(|w| w.contains(&column)) {\n                *slot",
        replacement: "            if wanted.is_none_or(|w| !w.is_empty()) {\n                *slot",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_varchar_column_has_a_lane_of_its_lengths"],
    },
    Mutation {
        name: "lane-pages-not-the-groups",
        package: Package::Core,
        breaks: "a group's pages leave out its lanes', so they are never freed",
        file: "src/engine/columns/group/wanted.rs",
        original: "self.columns.iter().chain(&self.lanes).flatten().flatten()",
        replacement: "self.columns.iter().flatten().flatten()",
        integration: &[],
        unit: &["engine::columns::group::tests::lanes::a_varchar_column_has_a_lane_of_its_lengths"],
    },
    Mutation {
        name: "lane-judged-of-a-measured-column-alone",
        package: Package::Core,
        breaks: "a comparison with the empty string is judged of a lane only of a measured column",
        file: "src/engine/columns/judged/lengths.rs",
        original: "    if !prepared.measures(pred.column) && group.lane(pred.column).is_none() {",
        replacement: "    if !prepared.measures(pred.column) {",
        integration: &[],
        unit: &["engine::columns::judged::lengths::tests::a_comparison_with_the_empty_string_is_judged_of_a_lane_where_one_is_kept"],
    },
    Mutation {
        name: "lane-lengths-held-of-any-column",
        package: Package::Core,
        breaks: "the lengths judged of a column the scan gives as text are held for a batch that takes none",
        file: "src/engine/columns/judged/lengths.rs",
        original: "    if given.is_none() && prepared.measures(pred.column) {",
        replacement: "    if given.is_none() {",
        integration: &[],
        unit: &["engine::columns::judged::lengths::tests::a_comparison_with_the_empty_string_is_judged_of_a_lane_where_one_is_kept"],
    },
    Mutation {
        name: "lane-proves-any-constant",
        package: Package::Core,
        breaks: "a lane's bounds prove a comparison with any constant, as one with the empty string",
        file: "src/engine/columns/judged/lengths.rs",
        original: "    octets.is_empty().then_some(*op)",
        replacement: "    Some(*op)",
        integration: &[],
        unit: &["engine::columns::judged::lengths::tests::a_lanes_bounds_prove_a_run_and_pass_over_parts"],
    },
    Mutation {
        name: "lane-parts-not-passed-over",
        package: Package::Core,
        breaks: "no part is passed over by its lane's bounds",
        file: "src/engine/columns/scan.rs",
        original: "    if let Some(runs) = lengths::kept_parts(group, condition) {",
        replacement: "    if let Some(runs) = lengths::kept_parts(group, condition).filter(|_| group.rows == 0) {",
        integration: &[],
        unit: &["engine::columns::judged::lengths::tests::a_lanes_bounds_prove_a_run_and_pass_over_parts"],
    },
];
