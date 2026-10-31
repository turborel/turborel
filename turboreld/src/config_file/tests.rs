//! Unit tests of the configuration file's subset of TOML (RULES.md
//! R-TEST-1, R-TEST-2): what it reads, each value as a setting's, and
//! what it refuses, where. Setting names are data to the parser, so most
//! are made up; those checked against the registry are the server's
//! own, by their constants. Split from config_file.rs (RULES.md R-SIZE-2).

use super::*;
use std::path::PathBuf;

use turborel::value::Decimal;
use turborel_server::settings::{HOST, PORT, WORKERS};

/// The entries `text` sets, each `NAME=VALUE`, as a value is shown.
fn read_as_text(text: &str) -> Vec<String> {
    read(text).unwrap().iter().map(|e| format!("{}={}", e.name, e.value)).collect()
}

/// The refusal of `text`: its line, its column, and its message.
fn refusal(text: &str) -> (usize, usize, String) {
    let refused = read(text).unwrap_err();
    (refused.at.line, refused.at.column, refused.message)
}

/// Verifies: [DESIGN] RULES.md D36: a file of nothing, blanks and
/// comments sets nothing, whatever its newlines, and a byte order mark
/// leading it is passed over.
#[test]
fn a_file_of_comments_sets_nothing() {
    for text in ["", "\n", "  \t\n", "# a comment\n\n   # another", "\r\n# x\r\n", "\u{feff}# marked\n"] {
        assert!(read(text).unwrap().is_empty(), "{text:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D36: a setting's name is the parts of its
/// header's key and its own joined by periods: bare, quoted and dotted
/// keys, blanks around the periods, a header for the pairs after it.
#[test]
fn keys_and_headers_name_settings() {
    let text = "TOP = 1\nA.B = 2\n\"C\" . 'D' = 3\n[ G ]\nX = 4 # after\nY.Z = 5\n[G2.\"H-I\"]\nk_1 = 6\n";
    let read_names: Vec<String> = read(text).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(read_names, ["TOP", "A.B", "C.D", "G.X", "G.Y.Z", "G2.H-I.k_1"]);
    let entries = read(text).unwrap();
    let at: Vec<(usize, usize)> = entries.iter().map(|e| (e.at.line, e.at.column)).collect();
    assert_eq!(at, [(1_usize, 1_usize), (2, 1), (3, 1), (5, 1), (6, 1), (8, 1)]);
    assert_eq!(read("  \tIndented = 1").unwrap()[0].at, At { line: 1_usize, column: 4_usize });
}

/// Verifies: [DESIGN] RULES.md D36: each value as a setting's: strings
/// basic and literal, integers with underscores and signs, floats,
/// booleans as the words ON and OFF, arrays of strings joined by commas.
#[test]
fn values_are_settings_values() {
    let text = "S = \"a b\"\nL = 'C:\\dir'\nI = 1_000_000\nN = -42\nP = +7\nZ = 0\nF = 0.5\nE = 1e3\nG = -2.5E-2\nU = 1_0.0_1e0_1\nT = true\nO = false\nA = [\"x\", 'y' ]\nM = []\n";
    let read_values = read_as_text(text);
    let expected = [
        "S=a b",
        "L=C:\\dir",
        "I=1000000",
        "N=-42",
        "P=7",
        "Z=0",
        "F=5E-1",
        "E=1E3",
        "G=-2.5E-2",
        "U=1.001E2",
        "T=ON",
        "O=OFF",
        "A=x,y",
        "M=",
    ];
    assert_eq!(read_values, expected);
    let entries = read("I = 9_223_372_036_854_775_807\n").unwrap();
    let Value::Exact(most) = &entries[0].value else { panic!("an exact number") };
    assert_eq!(most.to_string(), i64::MAX.to_string());
    assert!(matches!(read("F = 1.5").unwrap()[0].value, Value::Approx(_)));
}

/// Verifies: [DESIGN] RULES.md D36: a basic string's escapes, each of
/// TOML 1.0's, are the characters they stand for; a literal string's
/// reverse solidus is itself; a tab is taken as it is.
#[test]
fn escapes_are_read_in_basic_strings() {
    let entries = read("E = \"\\b\\t\\n\\f\\r\\\"\\\\\\u00E9\\U0001F600\ttab\"\nL = '\\n'").unwrap();
    let (Value::Char(basic), Value::Char(literal)) = (&entries[0].value, &entries[1].value) else { panic!("strings") };
    assert_eq!(basic, "\u{8}\t\n\u{c}\r\"\\\u{e9}\u{1f600}\ttab");
    assert_eq!(literal, "\\n");
}

/// Verifies: [DESIGN] RULES.md D36: an array may span lines, with
/// comments and a comma after its last string, and lines may end CR LF.
#[test]
fn an_array_spans_lines() {
    let text = "HOSTS = [\r\n  \"127.0.0.1\", # here\r\n  '::1',\n]\r\nNEXT = 1\r\n";
    assert_eq!(read_as_text(text), ["HOSTS=127.0.0.1,::1", "NEXT=1"]);
}

/// Verifies: [DESIGN] RULES.md D36: what TOML has beyond the subset is
/// refused, its line and column said: multi-line strings, other bases,
/// infinite and not-a-number floats, dates and times, inline tables,
/// arrays of tables, arrays of other than strings.
#[test]
fn what_is_beyond_the_subset_is_refused() {
    let cases: [(&str, usize, usize, &str); 12] = [
        ("A = \"\"\"x\"\"\"", 1, 5, "multi-line strings"),
        ("\nA = '''x'''", 2, 5, "multi-line strings"),
        ("A = 0x1F", 1, 5, "0x, 0o and 0b"),
        ("A = 0o7", 1, 5, "0x, 0o and 0b"),
        ("A = inf", 1, 5, "infinite and not-a-number"),
        ("A = -nan", 1, 5, "infinite and not-a-number"),
        ("A = 1979-05-27", 1, 5, "dates and times"),
        ("A = 07:32:00", 1, 5, "dates and times"),
        ("A = { B = 1 }", 1, 5, "inline tables"),
        ("[[T]]", 1, 1, "arrays of tables"),
        ("A = [1, 2]", 1, 6, "strings only"),
        ("A = [[\"x\"]]", 1, 6, "strings only"),
    ];
    for (text, line, column, message) in cases {
        let (at_line, at_column, said) = refusal(text);
        assert_eq!((at_line, at_column), (line, column), "{text}: {said}");
        assert!(said.contains(message), "{text}: {said}");
    }
}

/// Verifies: [DESIGN] RULES.md D36: a malformed key, value, string,
/// number or line is refused where it is, saying what was expected.
#[test]
fn malformed_lines_are_refused_where_they_are() {
    let cases: [(&str, usize, usize, &str); 20] = [
        ("A", 1, 2, "followed by ="),
        ("= 1", 1, 1, "a key is expected"),
        ("A. = 1", 1, 4, "a key is expected"),
        ("\"\" = 1", 1, 1, "not empty"),
        ("A = ", 1, 5, "a value is expected"),
        ("A = .5", 1, 5, "a value is expected"),
        ("A = word", 1, 5, "a string is quoted"),
        ("A = 1 2", 1, 7, "only a comment"),
        ("A = 1\r", 1, 7, "line feed"),
        ("A = \"open", 1, 5, "not closed"),
        ("A = 'open\n'", 1, 5, "not closed"),
        ("A = \"\\q\"", 1, 6, "\\q is not an escape"),
        ("A = \"\\u12\"", 1, 6, "4 hexadecimal digits"),
        ("A = \"\\uD800\"", 1, 6, "not a Unicode scalar value"),
        ("A = \"\u{7}\"", 1, 6, "U+0007 is a control character"),
        ("A = 1__0", 1, 5, "single underscores"),
        ("A = 01", 1, 5, "single underscores"),
        ("A = 1.", 1, 5, "not a number"),
        ("A = 9_223_372_036_854_775_808", 1, 5, "64 bits"),
        ("A = 1e999", 1, 5, "past the floats"),
    ];
    for (text, line, column, message) in cases {
        let (at_line, at_column, said) = refusal(text);
        assert_eq!((at_line, at_column), (line, column), "{text}: {said}");
        assert!(said.contains(message), "{text}: {said}");
    }
}

/// Verifies: [DESIGN] RULES.md D36: a header is a key in brackets, an
/// array's strings are separated by commas, hold none themselves, and
/// close; a comment holds no control character.
#[test]
fn headers_arrays_and_comments_are_refused_when_malformed() {
    let cases: [(&str, usize, usize, &str); 8] = [
        ("[G", 1, 3, "key in brackets"),
        ("[]", 1, 2, "a key is expected"),
        ("A = [\"x\" \"y\"]", 1, 10, "separated by commas"),
        ("A = [\"x,y\"]", 1, 6, "holds no comma"),
        ("A = [\"x\",\n", 1, 5, "not closed"),
        ("A = [\"x\"", 1, 5, "not closed"),
        ("A = [\r]", 1, 7, "line feed"),
        ("# \u{1b}", 1, 3, "control character"),
    ];
    for (text, line, column, message) in cases {
        let (at_line, at_column, said) = refusal(text);
        assert_eq!((at_line, at_column), (line, column), "{text}: {said}");
        assert!(said.contains(message), "{text}: {said}");
    }
}

/// Verifies: [DESIGN] RULES.md D36: a setting given twice, in any case,
/// by a dotted key or under a header, and a header given twice, are
/// refused at the second, the first's line said.
#[test]
fn a_setting_or_header_given_twice_is_refused() {
    assert_eq!(refusal("A.B = 1\n[A]\nb = 2"), (3_usize, 1_usize, "A.b is given twice, first at line 1".to_owned()));
    assert_eq!(refusal("[G]\nX = 1\n[g]"), (3_usize, 1_usize, "the table [g] is given twice, first at line 1".to_owned()));
    assert_eq!(refusal("[A.B]\n[\"A\".B]"), (2_usize, 1_usize, "the table [A.B] is given twice, first at line 1".to_owned()));
}

/// Verifies: [DESIGN] RULES.md D36: a refusal is shown `LINE:COLUMN:
/// message`, as the program prefixes it with the file's path.
#[test]
fn a_refusal_is_shown_with_its_place() {
    assert_eq!(read("\n  A").unwrap_err().to_string(), "2:4: a key is followed by = and its value");
}

/// A configuration of a database kept nowhere, with `file` and `set`.
fn config(file: &str, set: &[(&str, i128)]) -> Config {
    let settings = set.iter().map(|(name, n)| ((*name).to_owned(), Value::Exact(Decimal::new(*n, 0)))).collect();
    Config {
        store: crate::Store::Memory,
        settings,
        file: Some(PathBuf::from(file)),
        log: None,
        keys: None,
        owner: None,
        checksum_key: None,
    }
}

/// Verifies: [DESIGN] RULES.md D36, D42: the settings given are the
/// file's, each where it is, but those `--set` gives too, then `--set`'s,
/// so the command line wins; a file's name of no setting declared, the
/// database's or a server's, is refused with its path and line.
#[test]
fn the_command_line_wins_over_the_file() {
    let db = Database::new();
    let text = format!("{PORT} = 1\n{} = 2\n", WORKERS.to_lowercase());
    let kept = given(&db, &config("f.toml", &[(&format!(" {WORKERS} "), 3_i128)]), read(&text).unwrap()).unwrap();
    let shown: Vec<String> = kept.iter().map(|g| format!("{}={} {:?}", g.name, g.value, g.from)).collect();
    assert_eq!(shown, [format!("{PORT}=1 Some(\"f.toml:1:1\")"), format!(" {WORKERS} =3 None")]);
    let unknown = given(&db, &config("f.toml", &[]), read(&format!("{HOST} = []\nNO.SUCH = 1")).unwrap()).unwrap_err();
    assert_eq!(unknown, "f.toml:2:1: no setting is named NO.SUCH");
    let own = &db.settings().unwrap()[0].definition.name;
    let core = read(&format!("{own} = 1")).unwrap();
    assert_eq!(given(&db, &config("f.toml", &[]), core).unwrap().len(), 1_usize, "the database's own");
}
