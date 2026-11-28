//! The mutations of regular expression functions over vectors (RULES.md
//! R-TEST-9): the entries of a dictionary a function over its codes runs
//! (D1291), and what a searcher
//! leaves for the next (D1292).

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

/// The mutations of regular expression functions over vectors, in the order they run.
pub const REGEX_VECTORS: [Mutation; 10] = [
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
];
