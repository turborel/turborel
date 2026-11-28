//! The mutations of regular expression functions over vectors (RULES.md
//! R-TEST-9): the entries of a dictionary a function over its codes runs
//! (D1291), what a searcher leaves for the next (D1292), the steps of the
//! matcher that marks (D1311), and where a search stops or keeps no mark
//! (D1312).

use super::{Mutation, Package};

/// The file of a dictionary's entries a regular expression function runs.
const ENTRIES: &str = "src/functions/regex/entries.rs";
/// The unit test of a vector of codes against each row's call.
const CODES: &str = "functions::regex::tests::a_vector_of_codes_is_run_as_each_row_is";
/// The unit test of an entry no row reads.
const UNREAD: &str = "functions::regex::tests::an_entry_no_row_reads_is_not_run";
/// The unit test of a dictionary of more entries than rows.
const MORE_ENTRIES: &str = "functions::regex::tests::a_dictionary_of_more_entries_than_rows_is_run_a_row_at_a_time";
/// The unit test of what a searcher done leaves for the next.
const LEFT: &str = "regex::searcher::tests::a_searcher_takes_up_what_the_last_one_left";

/// The file of the matcher that marks.
const BOUNDED: &str = "src/regex/bounded.rs";
/// The file of what a program knows of each instruction's step.
const STEPS: &str = "src/regex/compile/steps.rs";
/// The unit test of the instructions the matcher that marks follows.
const FOLLOWED: &str = "regex::bounded::tests::marked_where_ways_meet_it_follows_each_instruction_at_a_position_once";
/// The unit test of marks set a word at a time.
const BY_WORDS: &str = "regex::bounded::tests::marks_set_a_word_at_a_time_are_those_set_one_by_one";
/// The unit test of a text read by its octets against its characters.
const BY_OCTETS: &str = "regex::bounded::tests::a_text_read_by_its_octets_is_matched_as_its_characters_are";

/// The file of the automaton of where matches end.
const DFA: &str = "src/regex/dfa.rs";
/// The unit test of a scan that stops where nothing can match.
const STOPS: &str = "regex::dfa::tests::a_scan_that_can_match_nothing_more_stops";
/// The unit test of a program of few ways.
const FEW_WAYS: &str = "regex::bounded::tests::a_program_of_few_ways_keeps_no_mark_and_finds_the_same";

/// The mutations of regular expression functions over vectors, in the order they run.
pub const REGEX_VECTORS: [Mutation; 25] = [
    Mutation {
        name: "regex-entries-translated-unread",
        package: Package::Core,
        breaks: "TRANSLATE_REGEX of a dictionary's codes translates entries no row reads, and raises of one (D1291)",
        file: "src/functions/regex/translate.rs",
        original: "given.push(match texts.get(n).filter(|_| wanted(n)) {",
        replacement: "given.push(match texts.get(n) {",
        integration: &["regex_functions::a_dictionary_column_is_translated_as_its_rows_are_6_33"],
        unit: &[UNREAD],
    },
    Mutation {
        name: "regex-entries-matched-unread",
        package: Package::Core,
        breaks: "LIKE_REGEX of a dictionary's codes matches entries no row reads, and raises of one (D1291)",
        file: "src/functions/regex.rs",
        original: "let entry = coded.entries.get(e).filter(|_| *wanted);",
        replacement: "let entry = coded.entries.get(e);",
        integration: &[],
        unit: &[UNREAD],
    },
    Mutation {
        name: "regex-entries-read-by-null-rows",
        package: Package::Core,
        breaks: "a row that is null marks its code's entry as read, so the entry is run (D1291)",
        file: ENTRIES,
        original: "if let Some(entry) = read.get_mut(index(code)).filter(|_| !subjects.is_null(n)) {",
        replacement: "if let Some(entry) = read.get_mut(index(code)) {",
        integration: &[],
        unit: &[UNREAD],
    },
    Mutation {
        name: "regex-entries-of-any-dictionary",
        package: Package::Core,
        breaks: "a dictionary of more entries than the vector has rows is run by entry (D1291)",
        file: ENTRIES,
        original: "coded.codes.len() == rows && coded.entries.len() <= rows",
        replacement: "coded.codes.len() == rows",
        integration: &[],
        unit: &[MORE_ENTRIES],
    },
    Mutation {
        name: "regex-entry-given-none-not-null",
        package: Package::Core,
        breaks: "a row whose entry was given a null is given the empty string (D1291)",
        file: ENTRIES,
        original: "if subjects.is_null(n) || given.get(index(code)) != Some(&true) {",
        replacement: "if subjects.is_null(n) {",
        integration: &[],
        unit: &[CODES],
    },
    Mutation {
        name: "regex-entry-of-another-row",
        package: Package::Core,
        breaks: "LIKE_REGEX of a dictionary's codes gives a row the truth value of the entry at the row's own place (D1291)",
        file: "src/functions/regex.rs",
        original: "coded.codes.get(n).is_some_and(|&code| found.get(index(code)) == Some(&true))",
        replacement: "coded.codes.get(n).is_some_and(|_| found.get(n) == Some(&true))",
        integration: &[],
        unit: &[CODES],
    },
    Mutation {
        name: "regex-searcher-takes-nothing-up",
        package: Package::Core,
        breaks: "a searcher of text after text takes up nothing a searcher left (D1292)",
        file: "src/regex.rs",
        original: "let left = self.left.take().unwrap_or_else(|| {",
        replacement: "let left = None.unwrap_or_else(|| {",
        integration: &[],
        unit: &[LEFT],
    },
    Mutation {
        name: "regex-searcher-leaves-nothing",
        package: Package::Core,
        breaks: "a searcher done leaves nothing for the next (D1292)",
        file: "src/regex/searcher.rs",
        original: "        if self.lasting {\n            self.regex.left.leave(",
        replacement: "        if false {\n            self.regex.left.leave(",
        integration: &[],
        unit: &[LEFT],
    },
    Mutation {
        name: "regex-searcher-forgets-what-it-found",
        package: Package::Core,
        breaks: "a searcher done leaves its automaton and forgets what its searches found (D1292)",
        file: "src/regex/searcher.rs",
        original: "self.regex.left.leave(Left { dfa: self.dfa.take(), found: self.found,",
        replacement: "self.regex.left.leave(Left { dfa: self.dfa.take(), found: Found::default(),",
        integration: &[],
        unit: &[LEFT],
    },
    Mutation {
        name: "regex-search-of-one-text-leaves",
        package: Package::Core,
        breaks: "a search of one text alone leaves a searcher with no automaton for the next (D1292)",
        file: "src/regex.rs",
        original: "kept: bounded::Scratch::default(), lasting: false }",
        replacement: "kept: bounded::Scratch::default(), lasting: true }",
        integration: &[],
        unit: &[LEFT],
    },
    Mutation {
        name: "regex-marks-nowhere",
        package: Package::Core,
        breaks: "the matcher that marks marks no instruction it follows, so its ways double at every fork (D1311)",
        file: BOUNDED,
        original: "let goes_on = !meets || self.mark(pc, at);",
        replacement: "let goes_on = true;",
        integration: &[],
        unit: &[FOLLOWED],
    },
    Mutation {
        name: "regex-jump-targets-unmarked",
        package: Package::Core,
        breaks: "an instruction a jump leads to is taken for one no two ways meet at (D1311)",
        file: STEPS,
        original: "Inst::Jump(to) => [Some(*to), None],",
        replacement: "Inst::Jump(_) => [None, None],",
        integration: &[],
        unit: &["regex::compile::steps::tests::two_ways_meet_where_a_fork_or_a_jump_leads"],
    },
    Mutation {
        name: "regex-range-marks-miss-a-set-bit",
        package: Package::Core,
        breaks: "marks set a word at a time take a position marked before for a new one (D1311)",
        file: BOUNDED,
        original: "        if met != 0 {\n            *held |= wanted & ones(",
        replacement: "        if false {\n            *held |= wanted & ones(",
        integration: &[],
        unit: &[BY_WORDS],
    },
    Mutation {
        name: "regex-range-marks-past-the-first",
        package: Package::Core,
        breaks: "marks set a word at a time mark the positions past the first marked before (D1311)",
        file: BOUNDED,
        original: "*held |= wanted & ones(usize::try_from(met.trailing_zeros()).unwrap_or(0));",
        replacement: "*held |= wanted;",
        integration: &[],
        unit: &[BY_WORDS],
    },
    Mutation {
        name: "regex-table-of-the-others",
        package: Package::Core,
        breaks: "an instruction's table holds the characters of ASCII it does not match (D1311)",
        file: STEPS,
        original: ".filter(|octet| set.contains(char::from(*octet)))",
        replacement: ".filter(|octet| !set.contains(char::from(*octet)))",
        integration: &[],
        unit: &["regex::compile::steps::tests::a_table_holds_the_ascii_characters_an_instruction_matches"],
    },
    Mutation {
        name: "regex-loop-ended-by-a-range",
        package: Package::Core,
        breaks: "a loop whose class leaves out a range of characters is ended by the range's first octet alone (D1311)",
        file: STEPS,
        original: ".filter(|octet| first == last && *octet < ASCII)",
        replacement: ".filter(|octet| *octet < ASCII)",
        integration: &[],
        unit: &["regex::compile::steps::tests::a_loop_of_every_character_but_one_of_ascii_is_ended_by_its_octet"],
    },
    Mutation {
        name: "regex-octet-pass-takes-any-ascii",
        package: Package::Core,
        breaks: "a loop run through passes every character of ASCII in its pass over the octets (D1311)",
        file: BOUNDED,
        original: "octets.iter().position(|octet| *octet >= PAST_ASCII || !holds(table, *octet))",
        replacement: "octets.iter().position(|octet| *octet >= PAST_ASCII)",
        integration: &[],
        unit: &[BY_OCTETS],
    },
    Mutation {
        name: "regex-table-step-takes-any-ascii",
        package: Package::Core,
        breaks: "a step of a text read by its octets takes any character of ASCII (D1311)",
        file: BOUNDED,
        original: "(Some(octet), Some(table)) => Ok(holds(table, octet)),",
        replacement: "(Some(_), Some(_)) => Ok(true),",
        integration: &[],
        unit: &[BY_OCTETS],
    },
    Mutation {
        name: "regex-scan-reads-on-with-nothing-left",
        package: Package::Core,
        breaks: "the automaton reads a text to its end though no thread is left and its end matches nothing (D1312)",
        file: DFA,
        original: "Some(if barren { DEAD } else { at })",
        replacement: "Some(at)",
        integration: &[],
        unit: &[STOPS],
    },
    Mutation {
        name: "regex-scan-stops-though-the-end-matches",
        package: Package::Core,
        breaks: "the automaton gives a text up where no thread is left though the text's end matches (D1312)",
        file: DFA,
        original: "let barren = threads.is_empty() && !self.closed(program, (&key.0, key.1), (key.2, true)).1;",
        replacement: "let barren = threads.is_empty();",
        integration: &[],
        unit: &[STOPS],
    },
    Mutation {
        name: "regex-straight-run-takes-any-octets",
        package: Package::Core,
        breaks: "a straight run compared at once takes whatever octets the text holds (D1312)",
        file: BOUNDED,
        original: "(within && wanted.is_some() && held == wanted)",
        replacement: "(within && wanted.is_some())",
        integration: &[],
        unit: &[FEW_WAYS],
    },
    Mutation {
        name: "regex-straight-run-past-a-meeting",
        package: Package::Core,
        breaks: "a straight run goes on past an instruction two ways can meet at (D1312)",
        file: STEPS,
        original: ".filter(|after| meets.get(*after) == Some(&false))",
        replacement: ".filter(|_| true)",
        integration: &[],
        unit: &["regex::compile::steps::tests::a_straight_run_ends_where_two_ways_can_meet"],
    },
    Mutation {
        name: "regex-few-ways-begun-anywhere",
        package: Package::Core,
        breaks: "a program whose match may begin anywhere is taken for one of few ways and keeps no mark (D1312)",
        file: STEPS,
        original: "anchored && ways.is_some_and(|ways| ways <= insts.len())",
        replacement: "ways.is_some_and(|ways| ways <= insts.len())",
        integration: &[],
        unit: &[FEW_WAYS],
    },
    Mutation {
        name: "regex-few-ways-of-any-forks",
        package: Package::Core,
        breaks: "a program's forks are not counted, so one of many ways keeps no mark (D1312)",
        file: STEPS,
        original: "Inst::Split(..) => forks = forks.saturating_add(1),",
        replacement: "Inst::Split(..) => {}",
        integration: &[],
        unit: &[FEW_WAYS],
    },
    Mutation {
        name: "regex-few-ways-with-a-loop",
        package: Package::Core,
        breaks: "a program with a loop it does not run through is taken for one of few ways (D1312)",
        file: STEPS,
        original: "                if *to <= pc {\n                    return false;\n                }",
        replacement: "",
        integration: &[],
        unit: &[FEW_WAYS],
    },
];
