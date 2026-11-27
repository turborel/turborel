//! How fast a regular expression is searched for (RULES.md D61, D1188): a
//! pattern with groups over texts of ASCII from a short address to a long
//! one, as a search of each text alone runs it, as a searcher kept across
//! the texts runs it, and asked only whether the text matches; and the
//! same of a text with no match. The lengths pass the marks a search
//! keeps, so the matcher of threads is measured beside the one that
//! marks. Run with `cargo xtask bench regex`, on the harness (RULES.md
//! R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::hint::black_box;

use harness::{Bench, Case};
use turborel::regex::{Ascii, Regex};

/// The lengths of the texts searched, in characters.
const LENGTHS: [usize; 7] = [64, 256, 1024, 4096, 16_384, 65_536, 262_144];
/// Characters searched a run.
const CHARS: usize = 8 * 1024 * 1024;

/// An address of `len` characters the pattern matches, or, if not
/// `matching`, one it does not.
fn address(len: usize, matching: bool) -> String {
    let mut text = String::from(if matching { "http://www.example.test/" } else { "ftp://www.example.test/" });
    while text.len() < len {
        text.push_str("path/of=some&words%20and_more/");
    }
    text.truncate(len);
    text
}

/// Searches `text` as many times as read `CHARS` characters by `one`,
/// which gives whether it matched: how many searches.
fn searched(text: &str, mut one: impl FnMut(&Ascii<'_>) -> bool) -> u64 {
    let ascii = Ascii::of(text).unwrap();
    let times = (CHARS / text.len()).max(1);
    let mut found = 0_u64;
    for _ in 0..times {
        found += u64::from(one(black_box(&ascii)));
    }
    black_box(found);
    u64::try_from(times).unwrap()
}

fn main() {
    let bench = Bench::new("regex");
    let regex = Regex::new("^https?://(www\\.)?([^/]+)/.*$", "s").unwrap();
    for len in LENGTHS {
        for matching in [true, false] {
            let text = address(len, matching);
            let kind = if matching { "a match" } else { "no match" };
            bench.measure(Case::new(format!("groups, each text alone, {kind}"), "searches").bytes(len), || {
                searched(&text, |t| regex.find_in(t, 0).unwrap().is_some())
            });
            let mut kept = regex.searcher();
            bench.measure(Case::new(format!("groups, a searcher kept, {kind}"), "searches").bytes(len), || {
                searched(&text, |t| kept.find_in(t, 0).unwrap().is_some())
            });
            let mut asked = regex.searcher();
            bench.measure(Case::new(format!("whether, a searcher kept, {kind}"), "searches").bytes(len), || {
                searched(&text, |t| asked.is_match_in(t).unwrap())
            });
        }
    }
}
