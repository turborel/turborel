//! The mutations of a LIKE judged of a dictionary's entries once each
//! (RULES.md R-TEST-9, D1253): when the entries are judged, each row
//! kept by its code, nulls left out, entries out of order refused, the
//! answer turned for NOT LIKE, and each entry matched alone where the
//! pattern has no piece to search the arena for.

use super::super::{Mutation, Package};

/// The file of a dictionary's entries judged.
const ENTRIES: &str = "src/engine/columns/judged/entries.rs";

/// The test that compares the entries judged with each row matched alone.
const ONCE: &str = "engine::columns::judged::entries::tests::a_dictionarys_entries_are_judged_once_each";
/// The test of when the entries are not judged.
const NOT_JUDGED: &str = "engine::columns::judged::entries::tests::a_dictionary_of_more_entries_than_rows_asked_is_not_judged_by_them";

/// Mechanisms whose removal must fail the requirements of a LIKE judged
/// of a dictionary's entries.
pub const LIKE_ENTRIES: [Mutation; 6] = [
    Mutation {
        name: "like-entries-of-fewer-rows",
        package: Package::Core,
        breaks: "every entry of a dictionary is matched for a few rows asked of it",
        file: ENTRIES,
        original: "    if these.len() < count {",
        replacement: "    if these.is_empty() {",
        integration: &[],
        unit: &[NOT_JUDGED],
    },
    Mutation {
        name: "like-entries-row-by-its-place",
        package: Package::Core,
        breaks: "a row is kept by the entry of its place in the chunk, not of its code",
        file: ENTRIES,
        original: "        if *kept.get(usize::try_from(get(codes, *bits, n)?).ok()?)? {",
        replacement: "        if *kept.get(n.checked_rem(count)?)? {",
        integration: &[],
        unit: &[ONCE],
    },
    Mutation {
        name: "like-entries-keep-nulls",
        package: Package::Core,
        breaks: "a null is kept by the entry its code names, where the predicate is unknown",
        file: ENTRIES,
        original: "        if chunk.is_null(n) {\n            continue;\n        }\n",
        replacement: "        if chunk.is_null(n) && these.is_empty() {\n            continue;\n        }\n",
        integration: &[],
        unit: &[ONCE],
    },
    Mutation {
        name: "like-entries-out-of-order-judged",
        package: Package::Core,
        breaks: "a dictionary whose entries do not lie one after another is judged, its rows left out, where decoding it raises an error",
        file: ENTRIES,
        original: "    if !ends.is_sorted() {",
        replacement: "    if ends.is_empty() {",
        integration: &[],
        unit: &[NOT_JUDGED],
    },
    Mutation {
        name: "like-entries-not-turned",
        package: Package::Core,
        breaks: "NOT LIKE keeps the entries the pattern matches",
        file: "src/storage/condition/liked.rs",
        original: "            *kept = *kept != self.not;",
        replacement: "            *kept = *kept != false;",
        integration: &[],
        unit: &[ONCE],
    },
    Mutation {
        name: "like-entries-alone-from-the-first",
        package: Package::Core,
        breaks: "an entry matched alone is read from the arena's start, so a first piece is found of every entry",
        file: "src/value/like/arena.rs",
        original: "                start = end;",
        replacement: "                start = 0;",
        integration: &[],
        unit: &[ONCE],
    },
];
