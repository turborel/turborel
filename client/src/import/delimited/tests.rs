use std::io::{self, BufReader, Read};

use super::*;

/// A field as read: its text, or none for null.
type Got = Option<String>;

/// The fields of each record of `text`, written as `dialect` says, and
/// the line each began on.
fn read(text: &[u8], dialect: Dialect) -> Result<Vec<(Vec<Got>, Position)>, SourceError> {
    let mut source = Delimited::new(text, dialect);
    let mut all = Vec::new();
    while let Some(record) = source.record()? {
        all.push((record.values.into_iter().map(got).collect(), record.position));
    }
    Ok(all)
}

/// `v`, a field's value, as read: a delimited source gives no other.
fn got(v: Value) -> Got {
    match v {
        Value::Null => None,
        Value::Char(s) => Some(s),
        other @ (Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_)) => {
            panic!("{other:?}")
        }
    }
}

/// The fields of each record of `text`.
fn fields(text: &str, dialect: Dialect) -> Vec<Vec<Got>> {
    read(text.as_bytes(), dialect).unwrap().into_iter().map(|(f, _)| f).collect()
}

/// `s` as a field read.
fn f(s: &str) -> Got {
    s.to_owned().into()
}

/// The error reading `text` gives.
fn error(text: &[u8], dialect: Dialect) -> SourceError {
    read(text, dialect).unwrap_err()
}

/// Verifies: [DESIGN] RULES.md D59 [RFC 4180 §2 rules 1, 2, 4-7]: CSV's
/// records end at CR LF or LF, the last's optional; fields split at
/// commas, spaces kept; a field in quotes holds commas, line breaks and
/// quotes written twice; an empty field is null, one of empty quotes the
/// empty string; an empty line is a record of one null field.
#[test]
fn csv_records_and_fields() {
    let csv = Dialect::csv;
    assert_eq!(fields("a,b\r\nc,d\n", csv()), [[f("a"), f("b")], [f("c"), f("d")]]);
    assert_eq!(fields("a,b\nc,d", csv()), [[f("a"), f("b")], [f("c"), f("d")]], "the last line break is optional");
    assert_eq!(fields(" a , b ", csv()), [[f(" a "), f(" b ")]], "spaces are part of a field");
    assert_eq!(fields("\"a,b\",\"say \"\"hi\"\"\"", csv()), [[f("a,b"), f("say \"hi\"")]]);
    assert_eq!(fields("\"one\r\ntwo\nthree\",x\n", csv()), [[f("one\r\ntwo\nthree"), f("x")]], "line breaks in quotes are kept");
    assert_eq!(fields(",\"\",", csv()), [[None, f(""), None]]);
    assert_eq!(fields("a\n\nb\n", csv()), [[f("a")], [None], [f("b")]]);
    assert_eq!(fields("\"\"\"\"", csv()), [[f("\"")]]);
    assert_eq!(fields("", csv()), Vec::<Vec<Got>>::new());
    assert_eq!(fields("é,\u{1F600}", csv()), [[f("é"), f("\u{1F600}")]]);
}

/// Verifies: [DESIGN] RULES.md D59: the text for null is null only not in
/// quotes; an empty field is then the empty string. A byte order mark
/// beginning the input is passed over, and nowhere else.
#[test]
fn csv_null_text_and_byte_order_mark() {
    let null = || Dialect::csv().with_null("NULL");
    assert_eq!(fields("NULL,\"NULL\",,null", null()), [[None, f("NULL"), f(""), f("null")]]);
    assert_eq!(fields("\u{FEFF}a,b\n\u{FEFF}c", Dialect::csv()), [vec![f("a"), f("b")], vec![f("\u{FEFF}c")]]);
}

/// Verifies: [DESIGN] RULES.md D59 [RFC 4180 §2 rule 3]: a header names
/// the fields, read once, whether or not asked for; records are numbered
/// from the first after it, and each says the line it began on, past the
/// line breaks of quoted fields; a header missing is refused.
#[test]
fn csv_header_and_positions() {
    let text = "ID,\"FULL NAME\"\n1,\"Ada\nLovelace\"\n2,Grace\n";
    let mut source = Delimited::new(text.as_bytes(), Dialect::csv().with_header());
    let headings = source.headings().unwrap().unwrap();
    let names: Vec<&str> = headings.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["ID", "FULL NAME"]);
    assert!(headings.iter().all(|h| h.data_type.is_none()));
    assert_eq!(source.headings().unwrap(), None, "read once");
    let positions: Vec<Position> = std::iter::from_fn(|| source.record().unwrap()).map(|r| r.position).collect();
    assert_eq!(positions, [Position { record: 1, line: Some(2) }, Position { record: 2, line: Some(4) }]);
    assert!(source.record().unwrap().is_none(), "none again after the end");
    // Not asked for, the header is passed over all the same.
    let skipped = fields(text, Dialect::csv().with_header());
    assert_eq!(skipped, [[f("1"), f("Ada\nLovelace")], [f("2"), f("Grace")]]);
    let missing = Delimited::new(&b""[..], Dialect::csv().with_header()).headings().unwrap_err();
    assert_eq!(missing.position, Position { record: 0, line: Some(1) });
    assert_eq!(missing.to_string(), "record 0, line 1: [DESIGN] there is no header");
    assert_eq!(Delimited::new(&b"a"[..], Dialect::csv()).headings().unwrap(), None, "no header unless told");
}

/// Verifies: [DESIGN] RULES.md D59 [RFC 4180 §2 rules 5, 7]: a quote in a
/// field not quoted, anything but the delimiter after a closing quote, a
/// quote left open at the end of the input, and a field not UTF-8 are
/// refused, saying the record, the line it began on, and the field; the
/// source then gives nothing more.
#[test]
fn csv_errors_say_where() {
    let quote = error(b"a\nb,c\"d\n", Dialect::csv());
    assert_eq!(quote.position, Position { record: 2, line: Some(2) });
    assert_eq!(quote.message, "[RFC 4180 §2 rule 5] field 2: a quote in a field not enclosed in quotes");
    let after = error(b"\"a\"b,c", Dialect::csv());
    assert_eq!(after.message, "[RFC 4180 §2 rule 5] field 1: a quoted field's closing quote is followed by more");
    let open = error(b"a\nb\n\"c\nd\ne\n", Dialect::csv());
    assert_eq!(open.position, Position { record: 3, line: Some(3) }, "where the record began");
    assert_eq!(open.message, "[RFC 4180 §2 rule 5] a quoted field is not closed");
    let open_last = error(b"\"c", Dialect::csv());
    assert_eq!(open_last.message, "[RFC 4180 §2 rule 5] a quoted field is not closed");
    let bytes = error(b"ok,\xFF\xFE", Dialect::csv());
    assert_eq!(bytes.to_string(), "record 1, line 1: [DESIGN] field 2 is not UTF-8");
    let tsv = error(b"ok\t\xFF", Dialect::tsv());
    assert_eq!(tsv.message, "[DESIGN] field 2 is not UTF-8");
    let mut source = Delimited::new(&b"\"a\"b\nc\n"[..], Dialect::csv());
    assert!(source.record().is_err());
    assert!(source.record().unwrap().is_none(), "nothing more after an error");
}

/// A reader that fails.
struct Failing;

impl Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("the disk is gone"))
    }
}

/// Verifies: [DESIGN] RULES.md D59: a record longer than the bound is
/// refused once the bound is read, so an endless input, or a quote never
/// closed, is read in bounded memory; a record of the bound is read. The
/// default bound is the protocol's longest body. A reader's failure is
/// told.
#[test]
fn records_are_bounded() {
    let endless = BufReader::new(io::repeat(b'a'));
    let mut source = Delimited::new(endless, Dialect::csv().with_max_record(1024));
    assert_eq!(source.record().unwrap_err().message, "[DESIGN] the record is longer than 1024 bytes");
    let open = BufReader::new(io::repeat(b'"').take(10_000_u64).chain(io::repeat(b'\n')));
    let mut quoted = Delimited::new(open, Dialect::csv().with_max_record(4096));
    assert_eq!(quoted.record().unwrap_err().message, "[DESIGN] the record is longer than 4096 bytes");
    assert_eq!(fields("abcd\n", Dialect::csv().with_max_record(5)), [[f("abcd")]], "its line break counted");
    assert!(read(b"abcde\n", Dialect::csv().with_max_record(5)).is_err());
    assert_eq!(DEFAULT_MAX_RECORD, usize::try_from(turborel_protocol::MAX_BODY).unwrap());
    let mut failing = Delimited::new(BufReader::new(Failing), Dialect::csv());
    assert_eq!(failing.record().unwrap_err().message, "[DESIGN] cannot read: the disk is gone");
}

/// Verifies: [DESIGN] RULES.md D59: tab-separated text splits at tabs,
/// with no quoting, its records at line breaks; an empty field is null.
#[test]
fn tab_separated_records() {
    let tsv = Dialect::tsv;
    assert_eq!(fields("a\tb\r\n\"c\t\"d\"\n", tsv()), [[f("a"), f("b")], [f("\"c"), f("\"d\"")]]);
    assert_eq!(fields("a\t\tb,c", tsv()), [[f("a"), None, f("b,c")]]);
    assert_eq!(fields("a\t\\N", tsv().with_null("\\N")), [[f("a"), None]]);
}

/// Verifies: [DESIGN] RULES.md D59: a delimiter is one ASCII character,
/// not a quote, CR or LF; fields then split at it.
#[test]
fn delimiters() {
    let semicolon = Dialect::csv().with_delimiter(';').unwrap();
    assert_eq!(fields("a;\"b;c\";d,e", semicolon), [[f("a"), f("b;c"), f("d,e")]]);
    let pipe = Dialect::tsv().with_delimiter('|').unwrap();
    assert_eq!(fields("a|b\tc", pipe), [[f("a"), f("b\tc")]]);
    for wrong in ['"', '\n', '\r', 'é'] {
        let refused = Dialect::csv().with_delimiter(wrong).unwrap_err();
        assert!(refused.starts_with("[DESIGN] a delimiter is one ASCII character"), "{refused}");
    }
}
