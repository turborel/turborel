//! The mutations of a LIKE judged by a column store's scan of each
//! chunk's texts where they lie (RULES.md R-TEST-9, D1251): the
//! pattern's machine over octets, its steps over codes, the steps kept
//! by their table, what a scan is told, how the scan judges it, and
//! what it keeps of a chunk it cannot judge.

use super::super::{Mutation, Package};

/// The file of a pattern's machine over octets.
const MACHINE: &str = "src/value/like/machine.rs";
/// The file of a machine's steps over codes.
const STEPPED: &str = "src/engine/columns/fsst/stepped.rs";
/// The file of the scan's judging of coded texts.
const CODED: &str = "src/engine/columns/judged/coded.rs";

/// The test that compares a machine with its pattern's pieces.
const AS_PIECES: &str = "value::like::machine::tests::a_machine_matches_as_the_pieces_do";
/// The test of patterns that have no machine.
const NO_MACHINE: &str = "value::like::machine::tests::a_pattern_of_an_arbitrary_character_or_too_long_has_no_machine";
/// The test that compares steps over codes with the pattern's matching.
const AS_TEXT: &str = "engine::columns::fsst::stepped::tests::steps_match_codes_as_the_pattern_matches_their_text";
/// The test of a piece inside a symbol.
const INSIDE: &str = "engine::columns::fsst::stepped::tests::a_piece_is_found_inside_a_symbol";
/// The test of an escaped octet.
const ESCAPED: &str = "engine::columns::fsst::stepped::tests::an_escaped_octet_steps_by_itself";
/// The test of malformed codes.
const MALFORMED: &str = "engine::columns::fsst::stepped::tests::malformed_codes_match_as_none";
/// The test of the steps a scan keeps.
const KEPT: &str = "engine::columns::fsst::stepped::tests::steps_are_made_once_a_pattern_and_a_table";
/// The test of a group judged in place.
const IN_PLACE: &str = "engine::columns::judged::coded::tests::a_like_is_judged_of_texts_coded_by_symbols_in_place";
/// The test of the order a LIKE is judged in.
const LAST: &str = "engine::columns::judged::coded::tests::a_like_is_judged_after_the_other_conjuncts";
/// The test of when a run is seen to be given whole.
const WHOLE: &str = "engine::columns::judged::coded::tests::a_run_is_seen_whole_by_the_rate_kept_of_a_share_of_it";
/// The test of where the scan gives way.
const GIVES_WAY: &str = "engine::columns::judged::coded::tests::a_like_gives_way_of_padded_strings_and_malformed_codes";
/// The test of what a scan is told of a LIKE.
const TOLD: &str = "logical::pushed::tests::a_like_of_a_constant_pattern_is_told_itself";

/// Mechanisms whose removal must fail the requirements of a LIKE judged
/// on codes.
pub const LIKE_CODED: [Mutation; 20] = [
    Mutation {
        name: "like-machine-falls-back-to-start",
        package: Package::Core,
        breaks: "the failure function: a piece that begins again inside itself is not found after a start of it that failed",
        file: MACHINE,
        original: "            back = usize::from(*after.get(row(back)?.start.checked_add(usize::from(octet))?)?);",
        replacement: "            back = base;",
        integration: &[],
        unit: &[AS_PIECES],
    },
    Mutation {
        name: "like-machine-accept-left",
        package: Package::Core,
        breaks: "a piece that ends inside a symbol before a last `%` is lost, the symbol's octets after it leaving the accepting state",
        file: MACHINE,
        original: "            after.get_mut(at.checked_mul(OCTETS)?..)?.get_mut(..OCTETS)?.fill(u8::try_from(at).ok()?);",
        replacement: "            after.get_mut(at.checked_mul(OCTETS)?..)?.get_mut(..OCTETS)?.fill(u8::try_from(at.checked_add(1)?).ok()?);",
        integration: &[],
        unit: &[INSIDE],
    },
    Mutation {
        name: "like-machine-accept-read-on",
        package: Package::Core,
        breaks: "a text known to match is read on, so a code of no symbol past where it matched refuses it",
        file: MACHINE,
        original: "            Some((at, at))",
        replacement: "            Some((at, at.checked_add(1)?))",
        integration: &[],
        unit: &[MALFORMED],
    },
    Mutation {
        name: "like-machine-last-piece-not-searched-on",
        package: Package::Core,
        breaks: "a last piece found before the value's end is not searched for again to its end",
        file: MACHINE,
        original: "            searched(after, at, piece, true)?;",
        replacement: "            searched(after, at, piece, false)?;",
        integration: &[],
        unit: &[AS_PIECES],
    },
    Mutation {
        name: "like-machine-of-any-length",
        package: Package::Core,
        breaks: "a pattern of more octets than an octet has states for is given a machine",
        file: MACHINE,
        original: "        let states = octets.checked_add(2).filter(|&n| n <= MOST_STATES)?;",
        replacement: "        let states = octets.checked_add(2)?;",
        integration: &[],
        unit: &[NO_MACHINE],
    },
    Mutation {
        name: "like-steps-symbol-by-its-last-octet",
        package: Package::Core,
        breaks: "a piece that ends inside a symbol is not found, the symbol stepping the machine by its last octet alone",
        file: STEPPED,
        original: "        Some(octets.iter().fold(state, |at, &octet| machine.step(at, octet)))",
        replacement: "        Some(octets.last().map_or(state, |&octet| machine.step(state, octet)))",
        integration: &[],
        unit: &[INSIDE],
    },
    Mutation {
        name: "like-steps-quoted-read-as-a-code",
        package: Package::Core,
        breaks: "the escape step: an octet quoted by the escape steps the machine as the code of that value",
        file: STEPPED,
        original: "                let half = if quoted { OCTETS } else { 0 };",
        replacement: "                let half = if quoted { 0 } else { 0 };",
        integration: &[],
        unit: &[AS_TEXT],
    },
    Mutation {
        name: "like-steps-quoted-escape-quotes",
        package: Package::Core,
        breaks: "a quoted octet of the escape's own value quotes the octet after it",
        file: STEPPED,
        original: "                quoted = !quoted && octet == ESCAPE;",
        replacement: "                quoted = octet == ESCAPE;",
        integration: &[],
        unit: &[ESCAPED],
    },
    Mutation {
        name: "like-steps-end-in-an-escape",
        package: Package::Core,
        breaks: "codes that end in an escape match or not, where decoding them raises an error",
        file: STEPPED,
        original: "        if state == self.malformed || (state < self.settled && quoted) {",
        replacement: "        if state == self.malformed {",
        integration: &[],
        unit: &[MALFORMED],
    },
    Mutation {
        name: "like-steps-kept-by-place-alone",
        package: Package::Core,
        breaks: "a chunk coded by another table of symbols is matched by the steps of the table before it",
        file: STEPPED,
        original: "        let same = found.and_then(|n| self.held.get(n)).is_some_and(|(_, of, _)| **of == *symbols);",
        replacement: "        let same = found.and_then(|n| self.held.get(n)).is_some();",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "like-scan-keeps-nulls",
        package: Package::Core,
        breaks: "a scan keeps a null by a LIKE, its codes none, where the predicate is unknown",
        file: CODED,
        original: "        if chunk.is_null(n) {\n            continue;\n        }\n        let kept = match by_steps {",
        replacement: "        let kept = match by_steps {",
        integration: &[],
        unit: &[IN_PLACE],
    },
    Mutation {
        name: "like-scan-not-turned",
        package: Package::Core,
        breaks: "a scan keeps by NOT LIKE the rows whose codes match the pattern",
        file: CODED,
        original: "            Some((texts, steps)) => texts.liked(n, steps)? != like.not(),",
        replacement: "            Some((texts, steps)) => texts.liked(n, steps)?,",
        integration: &[],
        unit: &[IN_PLACE],
    },
    Mutation {
        name: "like-scan-unjudged-chunk-left-out",
        package: Package::Core,
        breaks: "the rows of a chunk the scan cannot judge are left out, where the filter was to judge them",
        file: CODED,
        original: "        out.extend_from_slice(these);",
        replacement: "        out.extend_from_slice(&[]);",
        integration: &[],
        unit: &[GIVES_WAY],
    },
    Mutation {
        name: "like-scan-unjudged-chunk-twice",
        package: Package::Core,
        breaks: "a row judged before a chunk gave way is given twice",
        file: CODED,
        original: "        out.truncate(before);",
        replacement: "        out.truncate(out.len());",
        integration: &[],
        unit: &[GIVES_WAY],
    },
    Mutation {
        name: "like-scan-whole-run-judged-on",
        package: Package::Core,
        breaks: "a run seen to be given whole is judged to its end, though the filter judges every row of it again",
        file: CODED,
        original: "        if whole_by((out.len(), asked), (all.len(), rows)) {",
        replacement: "        if false {",
        integration: &[],
        unit: &[IN_PLACE],
    },
    Mutation {
        name: "like-scan-whole-by-a-few-rows",
        package: Package::Core,
        breaks: "a run is taken for whole by the first few rows judged of it",
        file: CODED,
        original: "    let sampled = asked.saturating_mul(KEPT_SHARE) >= all;",
        replacement: "    let sampled = asked.saturating_mul(KEPT_SHARE) >= asked;",
        integration: &[],
        unit: &[WHOLE],
    },
    Mutation {
        name: "like-scan-whole-by-the-rows-asked",
        package: Package::Core,
        breaks: "a run is taken for whole by a share of the rows asked of it, not of its rows",
        file: CODED,
        original: "    sampled && kept.saturating_mul(all).saturating_mul(KEPT_SHARE) >= asked.saturating_mul(rows)",
        replacement: "    sampled && kept.saturating_mul(all).saturating_mul(KEPT_SHARE) >= asked.saturating_mul(all)",
        integration: &[],
        unit: &[WHOLE],
    },
    Mutation {
        name: "like-kept-not-turned",
        package: Package::Core,
        breaks: "NOT LIKE is true of the strings the pattern matches",
        file: "src/storage/condition/liked.rs",
        original: "        self.pattern.matches(text) != self.not",
        replacement: "        self.pattern.matches(text)",
        integration: &[],
        unit: &[TOLD],
    },
    Mutation {
        name: "like-scan-judged-as-written",
        package: Package::Core,
        breaks: "a LIKE is judged of every row before the conjuncts that cost less a row",
        file: "src/engine/columns/judged.rs",
        original: "        preds.sort_by_key(Pred::rank);",
        replacement: "        preds.sort_by_key(|_| 0_u8);",
        integration: &[],
        unit: &[LAST],
    },
    Mutation {
        name: "like-told-not-negated",
        package: Package::Core,
        breaks: "NOT LIKE is told a scan as LIKE, so the scan leaves out the rows the filter keeps",
        file: "src/logical/pushed.rs",
        original: "            .or_else(|| liked(inner, true)),",
        replacement: "            .or_else(|| liked(inner, false)),",
        integration: &["coded_likes::a_like_is_judged_by_the_scan_before_other_columns_are_decoded_5_14"],
        unit: &[TOLD],
    },
];
