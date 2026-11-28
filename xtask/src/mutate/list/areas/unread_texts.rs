//! The mutations of the texts a scan makes and of those it does not
//! (RULES.md R-TEST-9): a run's texts gathered into one vector and
//! judged once, each end on a character's boundary, texts coded by
//! symbols decoded straight into it, and a vector made a string only
//! where its octets are judged (D1252); texts decoded through a block
//! (D1317) and the room of a run's texts by what the runs before gave
//! (D1318); a run's rows given codes once a
//! chunk shares entries, a dictionary's entries whole or those its rows
//! read, each once a chunk, one value one entry, and a LIKE searched in
//! the entries where they are no more than the rows asked (D1316); and a
//! column given measured, its lengths of its codes, its entries and its
//! nulls, of VARCHAR alone, left late at the stream's end, gathered with
//! its nulls, asked for where no filter reads its text, and the one
//! comparison its lengths tell, in the filter and in the scan (D1267).

use super::super::{Mutation, Package};

/// The file of a run's texts gathered.
const TEXTS: &str = "src/engine/columns/texts.rs";

/// The test of every encoding's rows gathered.
const EVERY: &str = "engine::columns::texts::tests::every_encoding_gives_its_rows_texts";
/// The test of a dictionary's entries given whole or by the rows' use.
const ENTRIES: &str = "engine::columns::texts::tests::a_dictionary_gives_its_entries_whole_or_those_its_rows_read";
/// The test of a run across chunks of several encodings.
const ACROSS: &str = "engine::columns::texts::tests::a_run_across_encodings_has_codes_once_a_chunk_shares_entries";
/// The test of texts decoded through a block.
const BLOCKS: &str = "engine::columns::fsst::tests::texts_longer_than_a_block_are_decoded_whole";
/// The test of the room made for a run's texts.
const ROOM: &str = "engine::columns::prepared::tests::mixed::the_room_of_a_runs_texts_follows_the_most_a_stored_octet_gave";
/// The test of a LIKE over a coded column's entries.
const LIKE_ENTRIES: &str = "exec::batch::judge::tests::a_like_over_entries_keeps_the_rows_their_texts_keep";
/// The test of octets that are no text.
const NOT_TEXT: &str = "engine::columns::texts::tests::octets_that_are_no_text_give_no_column";

/// The test of a measured column's comparison judged of its lengths.
const BY_LENGTHS: &str =
    "engine::columns::judged::lengths::tests::a_measured_columns_comparison_with_the_empty_string_is_judged_of_its_lengths";

/// Mechanisms whose removal must fail the requirements of a scan's texts,
/// gathered once a run, and of texts made only for a reader.
pub const UNREAD_TEXTS: [Mutation; 30] = [
    Mutation {
        name: "texts-ends-within-characters",
        package: Package::Core,
        breaks: "a run's octets judged UTF-8 together are given as texts though one ends within a character",
        file: TEXTS,
        original: "        if !ends.iter().all(|&end| text.is_char_boundary(end)) {",
        replacement: "        if !ends.iter().all(|&end| end <= text.len()) {",
        integration: &[],
        unit: &[NOT_TEXT],
    },
    Mutation {
        name: "texts-symbols-not-decoded",
        package: Package::Core,
        breaks: "a text coded by symbols adds no octets to its run's vector",
        file: TEXTS,
        original: "            decoder.decode(coded.codes_of(n)?, &mut into.octets)?;\n",
        replacement: "",
        integration: &[],
        unit: &[EVERY],
    },
    Mutation {
        name: "decoder-block-kept-full",
        package: Package::Core,
        breaks: "a block of decoded octets added to its vector is not begun again, so a long text is refused",
        file: "src/engine/columns/fsst.rs",
        original: "                out.extend_from_slice(self.block.get(..at)?);\n                at = 0;",
        replacement: "                out.extend_from_slice(self.block.get(..at)?);",
        integration: &[],
        unit: &[BLOCKS],
    },
    Mutation {
        name: "decoder-symbol-moves-on-by-the-longest",
        package: Package::Core,
        breaks: "a symbol's place in the block moves on by the longest symbol, not by its own length",
        file: "src/engine/columns/fsst.rs",
        original: ".copy_from_slice(word);\n            at = at.saturating_add(len);",
        replacement: ".copy_from_slice(word);\n            at = at.saturating_add(LONGEST);",
        integration: &[],
        unit: &[BLOCKS],
    },
    Mutation {
        name: "room-as-stored",
        package: Package::Core,
        breaks: "the room of a run's texts is what its chunks store whatever the runs before gave",
        file: "src/engine/columns/prepared.rs",
        original: "        usize::try_from(scaled).unwrap_or(stored).max(stored)",
        replacement: "        stored",
        integration: &[],
        unit: &[ROOM],
    },
    Mutation {
        name: "room-of-the-last-run",
        package: Package::Core,
        breaks: "a run that gave less a stored octet than one before lowers the room",
        file: "src/engine/columns/prepared.rs",
        original: "> wide(best.0).saturating_mul(wide(stored)) || best.1 == 0 {",
        replacement: "> wide(best.0).saturating_mul(wide(stored)) || best.1 < usize::MAX {",
        integration: &[],
        unit: &[ROOM],
    },
    Mutation {
        name: "text-made-unjudged",
        package: Package::Simd,
        breaks: "a vector of octets is made a string on AVX2 though they are not UTF-8",
        file: "simd/src/x86/utf8.rs",
        original: "    let valid = unsafe { avx2(&octets) };",
        replacement: "    let valid = unsafe { avx2(&octets) } || !octets.is_empty();",
        integration: &[],
        unit: &["tests::every_available_set_keeps_the_contract"],
    },
    Mutation {
        name: "texts-span-unshifted",
        package: Package::Core,
        breaks: "a field of a run of varying fields copied at once ends where it ends in its chunk, not past the run's start (D1131)",
        file: TEXTS,
        original: "            self.ends.push(at.checked_add(ends.checked_sub(start)?)?);",
        replacement: "            self.ends.push(at.checked_add(ends)?);",
        integration: &[],
        unit: &[EVERY],
    },
    Mutation {
        name: "texts-nulls-at-positions-dropped",
        package: Package::Core,
        breaks: "a null row asked at a position is given as a text",
        file: TEXTS,
        original: "places.iter().for_each(|&n| into.nulls.push(self.is_null(n))),",
        replacement: "places.iter().for_each(|_| into.nulls.push(false)),",
        integration: &[],
        unit: &[EVERY],
    },
    Mutation {
        name: "texts-rows-before-uncoded",
        package: Package::Core,
        breaks: "the rows gathered before a chunk that shares entries are all given the first entry's code",
        file: TEXTS,
        original: "            codes.extend(0..so_far);",
        replacement: "            codes.extend((0..so_far).map(|_| 0));",
        integration: &[],
        unit: &[ACROSS],
    },
    Mutation {
        name: "texts-entries-from-the-runs-first",
        package: Package::Core,
        breaks: "a row's code of a dictionary read after other chunks is taken among the run's first entries, not its chunk's",
        file: TEXTS,
        original: "                Some(base) => base.checked_add(u32::try_from(code).ok()?)?,",
        replacement: "                Some(_) => u32::try_from(code).ok()?,",
        integration: &[],
        unit: &[ACROSS],
    },
    Mutation {
        name: "texts-run-codes-from-the-runs-first",
        package: Package::Core,
        breaks: "the codes of a run with no null, taken in one loop, are not past the entries the run held before its chunk",
        file: TEXTS,
        original: "base.wrapping_add(u32::try_from(code).unwrap_or(0))",
        replacement: "u32::try_from(code).unwrap_or(0)",
        integration: &[],
        unit: &[NOT_TEXT],
    },
    Mutation {
        name: "texts-entries-whole-of-fewer-rows",
        package: Package::Core,
        breaks: "a dictionary asked for fewer rows than it has entries gives them all, so a run holds more entries than rows",
        file: TEXTS,
        original: "        let base = if asked.count() >= count {",
        replacement: "        let base = if asked.count() > 0 {",
        integration: &[],
        unit: &[ENTRIES],
    },
    Mutation {
        name: "texts-entry-given-a-row",
        package: Package::Core,
        breaks: "an entry a row before read of the chunk is added again for each row that reads it",
        file: TEXTS,
        original: "is_ok_and(|place| place >= first && place < self.ends.len())",
        replacement: "is_ok_and(|place| place >= first && place > self.ends.len())",
        integration: &[],
        unit: &[ENTRIES],
    },
    Mutation {
        name: "texts-entry-of-a-chunk-before",
        package: Package::Core,
        breaks: "a code is given the entry a chunk before gave that code",
        file: TEXTS,
        original: "is_ok_and(|place| place >= first && place < self.ends.len())",
        replacement: "is_ok_and(|place| place < self.ends.len())",
        integration: &[],
        unit: &[ENTRIES],
    },
    Mutation {
        name: "texts-one-value-an-entry-a-row",
        package: Package::Core,
        breaks: "a chunk of one value adds an entry for each of its rows",
        file: TEXTS,
        original: "            if held.is_none() {",
        replacement: "            if held.is_none() || n > 0 {",
        integration: &[],
        unit: &[ACROSS],
    },
    Mutation {
        name: "like-entries-none-kept",
        package: Package::Core,
        breaks: "a LIKE searched at once in a coded column's entries keeps no entry, so no row",
        file: "src/exec/batch/judge/texts.rs",
        original: "        let (entries, mut out) = (Mask::from_bools(&all), Vec::new());",
        replacement: "        let (entries, mut out) = (Mask::none(all.len()), Vec::new());",
        integration: &[],
        unit: &[LIKE_ENTRIES],
    },
    Mutation {
        name: "entries-judged-of-fewer-rows",
        package: Package::Core,
        breaks: "every entry of a coded column is judged though fewer rows are asked than it has entries",
        file: "src/exec/batch/judge/texts.rs",
        original: "    coded.entries.len() <= over.map_or(texts.len(), <[u32]>::len)",
        replacement: "    coded.entries.len() <= texts.len()",
        integration: &[],
        unit: &[LIKE_ENTRIES],
    },
    Mutation {
        name: "like-entry-by-the-rows-place",
        package: Package::Core,
        breaks: "a row's LIKE is read of the entry at the row's place, not at its code",
        file: "src/exec/batch/expr/compare.rs",
        original: "coded.codes.get(n).and_then(|&code| all.get(index(code)).copied())",
        replacement: "all.get(n).copied()",
        integration: &[],
        unit: &["exec::batch::expr::tests::like_is_judged_of_a_vector_s_text_or_of_its_entries"],
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
