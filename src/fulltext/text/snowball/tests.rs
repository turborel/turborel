//! Unit tests of the Snowball interpreter (RULES.md D320): programs read,
//! damaged ones refused at their line, and runs that would not end left
//! with the word as it was. The stemmers against Snowball's vocabularies
//! are in `text/tests/vocabulary.rs`.

use super::*;

/// The stem of `word` by the program `source`.
fn stemmed(source: &str, word: &str) -> String {
    Stemmer::compile(source).unwrap().stem(word)
}

/// Verifies: [DESIGN] RULES.md D320: a program's routines, groupings,
/// amongs and variables run as Snowball runs them, backward and forward,
/// an among taking its longest string.
#[test]
fn programs_run_as_snowball_runs_them() {
    let suffixes = "externals (stem)
        define stem as backwards ( [substring] among ( 'ing' 'ings' (delete) 'ed' (<- 'e') ) )";
    assert_eq!(stemmed(suffixes, "walkings"), "walk");
    assert_eq!(stemmed(suffixes, "walked"), "walke");
    assert_eq!(stemmed(suffixes, "walk"), "walk");
    let counted = "externals (stem) integers (n) define stem as ( $n = 0 repeat ( gopast 'a' $n += 1 ) $n >= 2 insert 'x' )";
    assert_eq!(stemmed(counted, "banana"), "bananax");
    assert_eq!(stemmed(counted, "cat"), "cat");
    let grouped = "externals (stem) groupings (v) define v 'aeiou' define stem as ( goto v [next] <- 'Y' )";
    assert_eq!(stemmed(grouped, "strap"), "strYp");
}

/// Verifies: [DESIGN] RULES.md D320: a program that is not Snowball, or
/// uses a name it never declares or defines, is refused as damaged
/// (SQLCODE -916) at its line.
#[test]
fn damaged_programs_are_refused() {
    for (source, line) in [
        ("externals (stem)\ndefine stem as (", "line 2"),
        ("externals (stem)\n\ndefine stem as ( call )", "line 3"),
        ("routines (other) define other as true", "line 1"),
        ("externals (stem) define stem as among ( 'a' 'a' )", "line 1"),
    ] {
        let e = Stemmer::compile(source).err().unwrap();
        assert_eq!(e.code, Code::Corrupt, "{source}");
        assert!(e.msg.contains(line), "{source}: {}", e.msg);
    }
    assert!(Stemmer::compile("externals (stem) define stem as ( 'unended )").is_err());
}

/// Verifies: [DESIGN] RULES.md D320: a run that recurses too deep or
/// loops too long stops, the word left as it was.
#[test]
fn runaway_runs_leave_the_word() {
    assert_eq!(stemmed("externals (stem) define stem as ( insert 'z' stem )", "word"), "word");
    assert_eq!(stemmed("externals (stem) define stem as ( insert 'z' repeat true )", "word"), "word");
}
