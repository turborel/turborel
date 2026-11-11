// Unit tests: a tailoring's rules are read as CLDR writes them (RULES.md
// D60).

use super::*;

fn relation(level: usize, text: &str, extension: &str) -> Relation {
    Relation { level, prefix: String::new(), text: text.to_owned(), extension: extension.to_owned() }
}

fn text(reset: &str) -> Reset {
    Reset::Text(reset.to_owned())
}

/// Verifies: [EXTENSION] RULES.md D60: a tailoring's rules are read as
/// CLDR writes them, levels, `[before n]`, extensions, quotes, escapes
/// and options among them.
#[test]
fn rules_are_read_as_cldr_writes_them() {
    let read = parse("[caseFirst upper]\n&[before 1]ǀ<å<<<Å # note\n&t<<<þ/h\n&C<c''h=c\\u2019h\n&D\\u0335 << đ").unwrap();
    assert!(read.upper_first && !read.backwards);
    assert_eq!(read.list[0], Rule { before: Some(1), reset: text("ǀ"), relations: vec![relation(1, "å", ""), relation(3, "Å", "")] });
    assert_eq!(read.list[1].relations, vec![relation(3, "þ", "h")]);
    assert_eq!(read.list[2].relations, vec![relation(1, "c'h", ""), relation(0, "c\u{2019}h", "")]);
    assert_eq!(read.list[3].reset, text("D\u{335}"));
    assert_eq!(read.list[3].relations, vec![relation(2, "đ", "")]);
    assert_eq!(parse("&'<'<b").unwrap().list[0].reset, text("<"));
}

/// Verifies: [EXTENSION] RULES.md D60: a list relation names each of its
/// characters in turn, a plain `-` the characters between two but a
/// quoted one itself, as is one with nothing before it; `<<<<` is the
/// fourth level; a prefix comes before `|`. MC/DC of a range, plain and
/// `-` and after a character: `d-f` against `'-'` (plain), against `b`
/// (`-`), and against `-b` (after a character).
#[test]
fn lists_ranges_prefixes_and_the_fourth_level_are_read() {
    let read = parse("&a<*bd-f'-'\\u0067\n&ゝ<<<<ヽ\n&[before 3]ぁ<<<ぁ|ー/x=あ|ー").unwrap();
    let listed: Vec<&str> = read.list[0].relations.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(listed, ["b", "d", "e", "f", "-", "g"]);
    assert!(read.list[0].relations.iter().all(|r| r.level == 1));
    let first: Vec<String> = parse("&a<*-b").unwrap().list[0].relations.iter().map(|r| r.text.clone()).collect();
    assert_eq!(first, ["-", "b"]);
    assert_eq!(read.list[1].relations, vec![relation(QUATERNARY, "ヽ", "")]);
    let prefixed = &read.list[2].relations;
    assert_eq!((prefixed[0].prefix.as_str(), prefixed[0].text.as_str(), prefixed[0].extension.as_str()), ("ぁ", "ー", "x"));
    assert_eq!((prefixed[1].level, prefixed[1].prefix.as_str(), prefixed[1].text.as_str()), (0, "あ", "ー"));
    assert_eq!(parse("&'\\u0020'=*'\\u3000'￣").unwrap().list[0].relations.len(), 2);
    // A direction mark is white space between tokens, but not quoted.
    assert_eq!(
        parse("\u{200E}&\u{200E}b\u{200F}<<\u{200E}c'\u{200E}'").unwrap().list[0],
        Rule { before: None, reset: text("b"), relations: vec![relation(2, "c\u{200E}", "")] }
    );
}

/// Verifies: [EXTENSION] RULES.md D60: a reset may name a position, and
/// the options of strength, reordering and suppressed contractions are
/// kept, the last `[reorder]` replacing those before it; `[optimize]`
/// and `[alternate shifted]` change nothing.
#[test]
fn positions_and_options_are_read() {
    let read = parse(
        "[strength 3][reorder Latn Kana][reorder Hani Bopo][suppressContractions [Ии\\u0439-\\u043A]]\n\
         [optimize [가-각]][alternate shifted]\n&[last regular]<一\n& [last primary ignorable]<<x\n\
         &[last secondary ignorable]<<<y\n&[last tertiary ignorable]=z",
    )
    .unwrap();
    assert_eq!(read.strength, 3);
    assert_eq!(read.reorder, ["Hani", "Bopo"]);
    assert_eq!(read.suppressed, ['И', 'и', 'й', 'к']);
    let resets: Vec<&Reset> = read.list.iter().map(|r| &r.reset).collect();
    let positions = [Last::Regular, Last::PrimaryIgnorable, Last::SecondaryIgnorable, Last::TertiaryIgnorable].map(Reset::Last);
    assert_eq!(resets, positions.iter().collect::<Vec<_>>());
    assert_eq!(parse("").unwrap(), Rules::default());
}

/// Verifies: [EXTENSION] RULES.md D60: what is not read is refused,
/// not half read.
#[test]
fn rules_not_read_are_refused() {
    for text in [
        "&a<<<<<b",
        "&a|b<c",
        "[import de standard]",
        "[strength 5]",
        "[alternate non-ignorable]",
        "&[first regular]<a",
        "<a",
        "&[before 4]a<b",
        "&a<'b",
        "&a<\\uZZZZ",
        "&a</",
        "&a<b|",
        "&a<*d-",
        "&a<*f-d",
        "&a]",
        "[reorder Latn",
        "[suppressContractions Ии]",
        "[suppressContractions [[a]]]",
    ] {
        assert_eq!(parse(text).map_err(|e| e.code), Err(Code::Corrupt), "{text}");
    }
}
