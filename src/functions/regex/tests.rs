//! Unit tests of LIKE_REGEX over a vector of values (RULES.md D1188),
//! of a subject read where it lies (RULES.md D1189), and of LIKE_REGEX
//! and TRANSLATE_REGEX over a vector of a dictionary's codes (RULES.md
//! D1291).

use super::translate::Translate;
use super::*;
use crate::error::SqlState;
use crate::storage::Coded;

/// A character literal's value.
fn chars(s: &str) -> Value {
    Value::Char(s.into())
}

/// Values of a column, the third null: with a match, without, the empty
/// string, one not of ASCII.
const VALUES: [&str; 6] = ["http://a.test/", "ftp://a.test/", "", "", "aéb", "a\r\nb"];

/// `VALUES` as a vector of texts.
fn values() -> Column {
    let mut ends = Vec::new();
    let text: String = VALUES
        .iter()
        .map(|v| {
            ends.push(ends.last().copied().unwrap_or(0_usize).saturating_add(v.len()));
            *v
        })
        .collect();
    let mut nulls = Mask::none(VALUES.len());
    nulls.set(2);
    Column::Texts(Texts::new(text, ends, Some(nulls)))
}

/// Verifies: [DESIGN] RULES.md D1188, D363 [ISO/IEC 9075-2:2023 §8.7
/// GR1]: LIKE_REGEX over a vector of values, its pattern and flag
/// constants, gives each row what a call of that row gives, Unknown of a
/// null value, and of every row if the pattern or the flag is null; a
/// pattern or a flag of each row has no call over vectors, and one that
/// is no regular expression fails as a row's call does.
#[test]
fn a_vector_of_values_is_matched_as_each_row_is() {
    let column = values();
    let row = |n: usize| if n == 2 { Value::Null } else { chars(VALUES[n]) };
    for (pattern, flag) in [("^https?://", ""), ("a.b", ""), ("a.b", "s"), ("^$", ""), ("é", ""), ("A", "i")] {
        let (pattern, flag, function) = (chars(pattern), chars(flag), LikeRegex::default());
        let made = function.eval_batch(&[Arg::Column(&column), Arg::Const(&pattern), Arg::Const(&flag)], VALUES.len()).unwrap().unwrap();
        for n in 0..VALUES.len() {
            let alone = function.call(&[row(n), pattern.clone(), flag.clone()]).unwrap();
            assert_eq!(format!("{:?}", made.values()[n]), format!("{alone:?}"), "{pattern:?} {flag:?} row {n}");
        }
    }
    let (pattern, flag, function) = (chars("a"), chars(""), LikeRegex::default());
    let nulled = function.eval_batch(&[Arg::Column(&column), Arg::Const(&Value::Null), Arg::Const(&flag)], 2).unwrap().unwrap();
    assert!(nulled.values().iter().all(Value::is_null) && nulled.values().len() == 2);
    let unknown = function.eval_batch(&[Arg::Column(&column), Arg::Const(&pattern), Arg::Const(&Value::Null)], 2).unwrap().unwrap();
    assert!(unknown.values().iter().all(Value::is_null));
    assert!(function.eval_batch(&[Arg::Column(&column), Arg::Column(&column), Arg::Const(&flag)], 2).is_none(), "a pattern of each row");
    assert!(function.eval_batch(&[Arg::Const(&pattern), Arg::Const(&pattern), Arg::Const(&flag)], 2).is_none(), "a constant value");
    let bad = chars("(");
    let failed = function.eval_batch(&[Arg::Column(&column), Arg::Const(&bad), Arg::Const(&flag)], 2).unwrap().unwrap_err();
    assert_eq!(failed.sqlstate(), function.call(&[row(0), bad.clone(), flag.clone()]).unwrap_err().sqlstate());
}

/// Verifies: [DESIGN] RULES.md D1189 [ISO/IEC 9075-2:2023 §6.31 GR18b
/// ii, §9.28 GR11b]: a subject not of ASCII is read where it lies, its
/// positions the octets its characters begin at. A start position in
/// characters names that character's position; one in octets the
/// character the octet is of, or the next if it is within one, and the
/// string's end within the last; one less than 1 or past the string
/// names none. A position is counted back in the units asked, from 1,
/// and what two positions span is the substring. A subject of ASCII
/// counts the same in either units.
#[test]
fn a_subject_read_where_it_lies_counts_in_its_units() {
    use super::subject::{Body, Subject};
    let value = chars("aébé€");
    let (characters, octets) = (chars(CHARACTERS), chars(OCTETS));
    let by_characters = Subject::new(&value, &characters).unwrap().unwrap();
    let by_octets = Subject::new(&value, &octets).unwrap().unwrap();
    assert!(matches!(by_characters.body, Body::Utf8(_)) && by_characters.len() == 9);
    let starts = |subject: &Subject<'_>, upto: i128| -> Vec<Option<usize>> { (0..=upto).map(|sp| subject.start(sp).unwrap()).collect() };
    assert_eq!(starts(&by_characters, 6), [None, Some(0), Some(1), Some(3), Some(4), Some(6), None]);
    let wanted = [None, Some(0), Some(1), Some(3), Some(3), Some(4), Some(6), Some(6), Some(9), Some(9), None];
    assert_eq!(starts(&by_octets, 10), wanted);
    assert_eq!((by_characters.position(0).unwrap(), by_characters.position(4).unwrap(), by_characters.position(9).unwrap()), (1, 4, 6));
    assert_eq!((by_octets.position(0).unwrap(), by_octets.position(4).unwrap(), by_octets.position(9).unwrap()), (1, 5, 10));
    assert_eq!(format!("{:?}", by_characters.substring(1, 4).unwrap()), format!("{:?}", chars("éb")));
    let plain = chars("abc");
    for units in [&characters, &octets] {
        let subject = Subject::new(&plain, units).unwrap().unwrap();
        assert!(matches!(subject.body, Body::Ascii(_)));
        assert_eq!((starts(&subject, 4), subject.position(2).unwrap()), (vec![None, Some(0), Some(1), Some(2), None], 3));
    }
    let none = chars("");
    assert_eq!(Subject::new(&none, &characters).unwrap().unwrap().start(1).unwrap(), None, "no first character");
}

/// A vector of the codes `codes` of a dictionary of `entries`, the rows
/// at `nulls` null.
fn coded(entries: &[&str], codes: &[u32], nulls: &[usize]) -> Column {
    let ends = (1..=entries.len()).map(|n| entries[..n].concat().len()).collect();
    let mut mask = Mask::none(codes.len());
    for &n in nulls {
        mask.set(n);
    }
    let entries = Arc::new(Texts::new(entries.concat(), ends, None));
    Column::Texts(Texts::coded(Coded { codes: codes.to_vec(), entries }, mask.any().then_some(mask)))
}

/// The strings `codes` name of `entries`, as a vector of texts one after
/// another, the rows at `nulls` null.
fn plain(entries: &[&str], codes: &[u32], nulls: &[usize]) -> Column {
    let strings: Vec<&str> = codes.iter().map(|&c| entries[usize::try_from(c).unwrap()]).collect();
    let ends = (1..=strings.len()).map(|n| strings[..n].concat().len()).collect();
    let mut mask = Mask::none(codes.len());
    for &n in nulls {
        mask.set(n);
    }
    Column::Texts(Texts::new(strings.concat(), ends, mask.any().then_some(mask)))
}

/// TRANSLATE_REGEX's operands but its subject: the units, ALL or
/// OCCURRENCE, the pattern, the flag, the replacement, the start position
/// and the occurrence.
type Asked = [Value; 7];

/// The operands of a TRANSLATE_REGEX of `pattern` under `flag` with
/// `replacement`, from the position `from` counted in `units`, of the
/// occurrence `occurrence` or of all.
fn asking((pattern, flag, replacement): (&str, &str, &str), (units, from, occurrence): (&str, i128, Option<i128>)) -> Asked {
    let every = if occurrence.is_some() { OCCURRENCE } else { crate::lexer::kw::ALL };
    [chars(units), chars(every), chars(pattern), chars(flag), chars(replacement), counted(from), counted(occurrence.unwrap_or(1))]
}

/// TRANSLATE_REGEX of the vector `subjects` as `asked` asks.
fn translated(asked: &Asked, subjects: &Column) -> Result<Column> {
    let [units, every, pattern, flag, replacement, from, occurrence] = asked;
    let consts = [units, every, pattern, flag].map(Arg::Const);
    let args = [
        consts[0],
        consts[1],
        consts[2],
        consts[3],
        Arg::Column(subjects),
        Arg::Const(replacement),
        Arg::Const(from),
        Arg::Const(occurrence),
    ];
    Translate::default().eval_batch(&args, subjects.values().len()).unwrap()
}

/// TRANSLATE_REGEX of the one subject `subject` as `asked` asks.
fn translated_alone(asked: &Asked, subject: &Value) -> Result<Value> {
    let [units, every, pattern, flag, replacement, from, occurrence] = asked.clone();
    Translate::default().call(&[units, every, pattern, flag, subject.clone(), replacement, from, occurrence])
}

/// Entries of a dictionary: links with and without a host, the empty
/// string, strings not of ASCII, a CR before an LF, a string no row reads.
const ENTRIES: [&str; 9] = [
    "http://www.a.test/x",
    "https://b.test/y/z",
    "ftp://c.test/",
    "",
    "aéb/é",
    "http://d.test",
    "a\r\nb",
    "http://www.a.test/x ",
    "never read/",
];

/// Rows of `ENTRIES`, most entries read more than once, the last by none.
const CODES: [u32; 14] = [0, 1, 0, 3, 4, 2, 5, 6, 7, 0, 4, 1, 3, 6];

/// Verifies: [DESIGN] (RULES.md D1291) [ISO/IEC 9075-2:2023 §6.33 GR9,
/// §8.7 GR1] TRANSLATE_REGEX and LIKE_REGEX over a vector of a
/// dictionary's codes give each row what a call of that row's string
/// gives, and what the same strings as texts one after another give: a
/// null of a null row, and of a row whose start position is not of its
/// string. TRANSLATE_REGEX's rows keep their codes, of entries each
/// translated once. So of every match or one occurrence, a start
/// position, either units, and a pattern whose match of no characters
/// fails every row alike.
#[test]
fn a_vector_of_codes_is_run_as_each_row_is() {
    let nulls = [2_usize, 11];
    let (by_codes, by_texts) = (coded(&ENTRIES, &CODES, &nulls), plain(&ENTRIES, &CODES, &nulls));
    let row = |n: usize| by_texts.values()[n].clone();
    let host = ("^https?://(www\\.)?([^/]+)/.*$", "s", "$2");
    let patterns = [host, ("a", "", "[$0]"), ("é", "", "e"), ("^(.)(.*)$", "s", "$2$1"), ("/", "q", "$"), ("t+", "i", ""), ("x*", "", "-")];
    let ways = [(CHARACTERS, 1, None), (CHARACTERS, 4, None), (OCTETS, 4, Some(2)), (CHARACTERS, 1, Some(1)), (OCTETS, 30, None)];
    for (pattern, way) in patterns.iter().flat_map(|pattern| ways.iter().map(move |way| (*pattern, *way))) {
        let asked = asking(pattern, way);
        let (of_codes, of_texts) = (translated(&asked, &by_codes), translated(&asked, &by_texts));
        for n in 0..CODES.len() {
            let (alone, what) = (translated_alone(&asked, &row(n)), format!("{pattern:?} {way:?} row {n}"));
            match (&of_codes, &of_texts) {
                (Ok(by_code), Ok(by_text)) => {
                    assert_eq!(format!("{:?}", by_code.values()[n]), format!("{:?}", alone.unwrap()), "{what}");
                    assert_eq!(format!("{:?}", by_text.values()[n]), format!("{:?}", by_code.values()[n]), "{what}");
                }
                (Err(by_code), Err(by_text)) => {
                    assert_eq!(pattern.0, "x*", "{what}");
                    assert_eq!(
                        (by_code.sqlstate(), by_text.sqlstate()),
                        (SqlState::ATTEMPT_TO_REPLACE_A_ZERO_LENGTH_STRING, SqlState::ATTEMPT_TO_REPLACE_A_ZERO_LENGTH_STRING)
                    );
                }
                (Ok(_), Err(_)) | (Err(_), Ok(_)) => panic!("{what}: codes and texts differ"),
            }
        }
        if let Ok(Column::Texts(made)) = &of_codes {
            assert_eq!(made.coded.as_ref().map(|c| c.codes.clone()), Some(CODES.to_vec()), "the rows keep their codes");
        }
    }
    for (pattern, flag) in [("^https?://", ""), ("a.b", ""), ("a.b", "s"), ("^$", ""), ("é", ""), ("TEST", "i")] {
        let (pattern, flag, function) = (chars(pattern), chars(flag), LikeRegex::default());
        let of = |column: &Column| {
            function.eval_batch(&[Arg::Column(column), Arg::Const(&pattern), Arg::Const(&flag)], CODES.len()).unwrap().unwrap()
        };
        let (of_codes, of_texts) = (of(&by_codes), of(&by_texts));
        for n in 0..CODES.len() {
            let alone = function.call(&[row(n), pattern.clone(), flag.clone()]).unwrap();
            assert_eq!(format!("{:?}", of_codes.values()[n]), format!("{alone:?}"), "{pattern:?} {flag:?} row {n}");
            assert_eq!(format!("{:?}", of_texts.values()[n]), format!("{alone:?}"), "{pattern:?} {flag:?} row {n} of texts");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1291) an entry of a dictionary that no
/// row reads, or only a row that is null, is not run. A row filtered out
/// before the call leaves its entry behind, and the call of that entry
/// may raise what no row of the query raises. Of TRANSLATE_REGEX that is
/// a result longer than the longest string, SQLSTATE 22001, and of
/// LIKE_REGEX a search with a back-reference past its steps, SQLCODE
/// -900. A row that does read the entry raises it, as its own call does.
#[test]
fn an_entry_no_row_reads_is_not_run() {
    let (long, hostile) = ("a".repeat(100), "a".repeat(60));
    let with = "x".repeat(400);
    let asked = asking(("a", "", with.as_str()), (CHARACTERS, 1, None));
    let entries = ["b", long.as_str(), "ab"];
    let unread = translated(&asked, &coded(&entries, &[0, 2, 0, 2], &[])).unwrap();
    assert_eq!(unread.values().len(), 4);
    assert_eq!(format!("{:?}", unread.values()[1]), format!("{:?}", translated_alone(&asked, &chars("ab")).unwrap()));
    let of_a_null = translated(&asked, &coded(&entries, &[0, 1, 2], &[1])).unwrap();
    assert!(of_a_null.values()[1].is_null() && !of_a_null.values()[2].is_null());
    let read = translated(&asked, &coded(&entries, &[0, 1, 2], &[])).unwrap_err();
    assert_eq!(read.sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
    assert_eq!(read.sqlstate(), translated_alone(&asked, &chars(&long)).unwrap_err().sqlstate());

    let (pattern, flag, function) = (chars("(a|aa)*\\1c"), chars(""), LikeRegex::default());
    let words = ["b", hostile.as_str(), "aac"];
    let of = |codes: &[u32], nulls: &[usize]| {
        function.eval_batch(&[Arg::Column(&coded(&words, codes, nulls)), Arg::Const(&pattern), Arg::Const(&flag)], codes.len()).unwrap()
    };
    let skipped = of(&[0, 2, 2, 0], &[]).unwrap();
    assert_eq!(
        format!("{:?}", skipped.values()),
        format!("{:?}", [Value::Bool(false), Value::Bool(true), Value::Bool(true), Value::Bool(false)])
    );
    assert!(of(&[1, 0, 2], &[0]).unwrap().values()[0].is_null());
    assert_eq!(of(&[1, 0, 2], &[]).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] (RULES.md D1291) a dictionary of more entries than
/// the vector has rows is run a row at a time, its entries marked for
/// nothing otherwise, and gives texts one after another, each row's as
/// its call gives it. One of as many entries as rows is run by entry.
#[test]
fn a_dictionary_of_more_entries_than_rows_is_run_a_row_at_a_time() {
    let asked = asking(("^https?://(www\\.)?([^/]+)/.*$", "s", "$2"), (CHARACTERS, 1, None));
    let is_coded = |column: &Column| if let Column::Texts(texts) = column { texts.coded.is_some() } else { false };
    for (rows, by_entry) in [(8, false), (9, true), (12, true)] {
        let codes: Vec<u32> = (0..rows).map(|n| [0, 1, 4, 2][n % 4]).collect();
        let made = translated(&asked, &coded(&ENTRIES, &codes, &[])).unwrap();
        assert_eq!(is_coded(&made), by_entry, "{rows} rows of 9 entries");
        for (n, &code) in codes.iter().enumerate() {
            let alone = translated_alone(&asked, &chars(ENTRIES[usize::try_from(code).unwrap()])).unwrap();
            assert_eq!(format!("{:?}", made.values()[n]), format!("{alone:?}"), "{rows} rows, row {n}");
        }
    }
    assert!(!is_coded(&translated(&asked, &plain(&ENTRIES, &CODES, &[])).unwrap()), "texts one after another stay so");
    let subject = coded(&ENTRIES, &CODES, &[]);
    let [units, every, pattern, flag, _, from, occurrence] = &asked;
    let consts = [units, every, pattern, flag].map(Arg::Const);
    let per_row = [
        consts[0],
        consts[1],
        consts[2],
        consts[3],
        Arg::Column(&subject),
        Arg::Column(&subject),
        Arg::Const(from),
        Arg::Const(occurrence),
    ];
    assert!(Translate::default().eval_batch(&per_row, CODES.len()).is_none(), "a replacement of each row has no call over vectors");
}
