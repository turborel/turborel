//! The mutations of texts a scan does not make because nobody reads them
//! (RULES.md R-TEST-9): a dictionary chunk's entries made text on first
//! use and of character strings alone (D1298).

use super::super::{Mutation, Package};

/// The file of the chunks a scan opens once.
const PREPARED: &str = "src/engine/columns/prepared.rs";

/// The test of when a dictionary's entries are made text.
const FIRST_ASKED: &str = "engine::columns::prepared::tests::a_dictionarys_entries_are_made_text_when_first_asked_for";
/// The test of entries that are no text.
const NOT_TEXT: &str = "engine::columns::prepared::tests::entries_not_text_give_none_however_late_they_are_made";

/// Mechanisms whose removal must fail the requirements of texts made
/// only for a reader.
pub const UNREAD_TEXTS: [Mutation; 2] = [
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
];
