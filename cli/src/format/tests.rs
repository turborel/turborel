//! Tests of the output styles.

use std::io;

use turborel::value::{DataType, Decimal, Spelled};

use super::*;

fn column(name: Option<&str>, data_type: DataType) -> Column {
    Column { name: name.map(str::to_owned), data_type, not_null: false }
}

/// Two columns, a name and a number, and two rows, one of a null.
fn written(layout: &Layout, screen: Option<usize>, expanded: bool) -> String {
    let mut out = Vec::new();
    let mut p = Printer::new(&mut out, layout, screen, expanded);
    p.columns(&[
        column(Some("NAME"), DataType::character(6)),
        column(Some("N"), DataType::Exact { precision: 3, scale: 0, spelled: Spelled::Numeric }),
    ]);
    p.rows(&[
        vec![Value::Char("a, \"b\"".into()), Value::Exact(Decimal::new(7, 0))],
        vec![Value::Null, Value::Exact(Decimal::new(-12, 0))],
    ]);
    p.finish(2, None).unwrap();
    String::from_utf8(out).unwrap()
}

fn layout(style: Style) -> Layout {
    Layout { style, null: "?".into(), ..Layout::default() }
}

/// Verifies: [DESIGN] RULES.md D46: a column is as wide as its type's
/// values, at most `MAX_WIDTH`, or its name; numbers align right, text
/// left; a null is shown as asked; the count ends the table.
#[test]
fn a_table_is_shaped_by_its_types() {
    let mut out = Vec::new();
    let given = Layout { null: "?".into(), ..Layout::default() };
    let mut table = Printer::new(&mut out, &given, None, false);
    let columns = [
        column(Some("NAME"), DataType::character(3)),
        column(Some("N"), DataType::Exact { precision: 3, scale: 1, spelled: Spelled::Numeric }),
        column(None, DataType::Approximate { precision: 53, spelled: Spelled::Float }),
        column(Some("LONG"), DataType::character(32767)),
    ];
    table.columns(&columns);
    let row = vec![Value::Char("ab".into()), Value::Exact(Decimal::new(-125, 1)), Value::Null, Value::Char("x".into())];
    table.rows(&[row]);
    table.finish(1, None).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0], format!(" NAME |     N | {:24} | LONG", ""));
    assert_eq!(lines[1], format!("------+-------+{}+{}", "-".repeat(26), "-".repeat(crate::show::MAX_WIDTH + 2)));
    assert_eq!(lines[2], format!(" ab   | -12.5 | {:>24} | x", "?"));
    assert_eq!(lines[3], "(1 row)");
}

/// Verifies: [DESIGN] RULES.md D46; [ISO/IEC 9075:1992 §22.1] a query
/// that completed with a warning says so after its count, with its
/// SQLSTATE.
#[test]
fn a_warning_follows_the_count() {
    let mut out = Vec::new();
    let given = Layout::default();
    let mut table = Printer::new(&mut out, &given, None, false);
    table.columns(&[column(Some("A"), DataType::character(1))]);
    table.rows(&[vec![Value::Char("a".into())], vec![Value::Char("b".into())]]);
    table.finish(2, Some("01004")).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.lines().last(), Some("(2 rows, with a warning: SQLSTATE 01004)"));
}

/// Verifies: [DESIGN] RULES.md D46: a failure to write is kept, and
/// nothing more is written after it.
#[test]
fn a_failure_to_write_is_reported() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Broken;
    let given = Layout::default();
    let mut table = Printer::new(&mut out, &given, None, false);
    table.columns(&[column(Some("A"), DataType::character(1))]);
    table.rows(&[vec![Value::Char("a".into())]]);
    assert_eq!(table.finish(2, None).unwrap_err().to_string(), "closed");
}

/// Verifies: [DESIGN] RULES.md D213: borders 0 and 2, the rows alone,
/// and no footer.
#[test]
fn borders_rows_alone_and_footers() {
    let b0 = Layout { border: 0, ..layout(Style::Aligned) };
    assert_eq!(written(&b0, None, false), "NAME       N\n------ -----\na, \"b\"     7\n?        -12\n(2 rows)\n");
    let b2 = Layout { border: 2, ..layout(Style::Aligned) };
    assert_eq!(
        written(&b2, None, false),
        "+--------+-------+\n| NAME   |     N |\n+--------+-------+\n| a, \"b\" |     7 |\n| ?      |   -12 |\n+--------+-------+\n(2 rows)\n"
    );
    let alone = Layout { tuples_only: true, ..layout(Style::Aligned) };
    assert_eq!(written(&alone, None, false), " a, \"b\" |     7\n ?      |   -12\n");
    let bare = Layout { footer: false, ..layout(Style::Aligned) };
    assert!(!written(&bare, None, false).contains("rows"));
}

/// Verifies: [DESIGN] RULES.md D213: expanded rows, a line naming each
/// and a line for each column; automatic expansion only for rows wider
/// than the screen.
#[test]
fn rows_are_expanded() {
    let on = layout(Style::Aligned);
    assert_eq!(written(&on, None, true), "-[ ROW 1 ]---\nNAME | a, \"b\"\nN    | 7\n-[ ROW 2 ]\nNAME | ?\nN    | -12\n(2 rows)\n");
    let auto = Layout { expanded: Expanded::Auto, ..layout(Style::Aligned) };
    assert!(written(&auto, Some(10), false).starts_with("-[ ROW 1 ]"));
    assert!(written(&auto, Some(80), false).starts_with(" NAME"));
    assert!(written(&auto, None, false).starts_with(" NAME"), "no screen, no expansion");
}

/// Verifies: [DESIGN] RULES.md D213: wrapped columns are made to fit the
/// screen, their values wrapped within them.
#[test]
fn wrapped_columns_fit_the_screen() {
    let wrapped = layout(Style::Wrapped);
    assert_eq!(written(&wrapped, Some(11), false), " NAME |   N\n------+-----\n a, \" |   7\n b\"   |\n ?    | -12\n(2 rows)\n");
    assert_eq!(written(&wrapped, None, false), written(&layout(Style::Aligned), None, false), "no screen, aligned");
}

/// Verifies: [DESIGN] RULES.md D213, D219: unaligned values apart by the
/// separator; CSV quoted as RFC 4180 says, without a footer; tab-separated
/// values escaped; each expanded.
#[test]
fn values_apart_by_separators() {
    let un = Layout { separator: ";".into(), ..layout(Style::Unaligned) };
    assert_eq!(written(&un, None, false), "NAME;N\na, \"b\";7\n?;-12\n(2 rows)\n");
    assert_eq!(written(&un, None, true), "NAME;a, \"b\"\nN;7\n\nNAME;?\nN;-12\n\n(2 rows)\n");
    assert_eq!(written(&layout(Style::Csv), None, false), "NAME,N\n\"a, \"\"b\"\"\",7\n?,-12\n");
    assert_eq!(written(&layout(Style::Csv), None, true), "NAME,\"a, \"\"b\"\"\"\nN,7\n\nNAME,?\nN,-12\n");
    assert_eq!(text::escaped("a\tb\\c\nd\re"), "a\\tb\\\\c\\nd\\re");
    assert_eq!(written(&layout(Style::Tabs), None, false), "NAME\tN\na, \"b\"\t7\n?\t-12\n");
    let alone = Layout { tuples_only: true, ..layout(Style::Csv) };
    assert_eq!(written(&alone, None, false), "\"a, \"\"b\"\"\",7\n?,-12\n");
}

/// Verifies: [DESIGN] RULES.md D213: HTML, its text escaped, numbers
/// aligned right, the footer after; expanded, a pair of cells a column.
#[test]
fn html_tables() {
    let shown = written(&layout(Style::Html), None, false);
    assert_eq!(
        shown,
        "<table border=\"1\">\n  <tr><th align=\"left\">NAME</th><th align=\"right\">N</th></tr>\n  \
         <tr><td align=\"left\">a, &quot;b&quot;</td><td align=\"right\">7</td></tr>\n  \
         <tr><td align=\"left\">?</td><td align=\"right\">-12</td></tr>\n</table>\n<p>(2 rows)</p>\n"
    );
    let expanded = written(&layout(Style::Html), None, true);
    assert!(expanded.contains("<tr><th colspan=\"2\">Row 2</th></tr>"), "{expanded}");
    assert!(expanded.contains("<tr><td>N</td><td align=\"right\">-12</td></tr>"), "{expanded}");
    assert_eq!(markup::html("<a&b>"), "&lt;a&amp;b&gt;");
}

/// Verifies: [DESIGN] RULES.md D213: JSON, an object a row, a number as a
/// number, a null as null, text as a string escaped; a number JSON cannot
/// write as one, as a string.
#[test]
fn json_arrays() {
    assert_eq!(written(&layout(Style::Json), None, false), "[\n{\"NAME\":\"a, \\\"b\\\"\",\"N\":7},\n{\"NAME\":null,\"N\":-12}\n]\n");
    assert_eq!(markup::json_string("a\u{1}\n"), "\"a\\u0001\\n\"");
    let mut out = Vec::new();
    let given = layout(Style::Json);
    let mut p = Printer::new(&mut out, &given, None, false);
    p.columns(&[column(Some("F"), DataType::Approximate { precision: 53, spelled: Spelled::Float }), column(Some("B"), DataType::Boolean)]);
    p.rows(&[vec![Value::Approx(f64::INFINITY), Value::Bool(true)]]);
    p.finish(1, None).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("\"B\":true"), "{text}");
    assert!(!text.contains("\"F\":inf") && !text.contains("\"F\":Inf"), "{text}");
}

/// Verifies: [DESIGN] RULES.md D213: each option is set and read back by
/// name; a wrong name or value is refused.
#[test]
fn options_are_set_by_name() {
    let mut l = Layout::default();
    for (option, value) in [
        ("style", "csv"),
        ("expanded", "auto"),
        ("border", "2"),
        ("null", "-"),
        ("separator", ";"),
        ("tuples", "on"),
        ("footer", "off"),
        ("pager", "always"),
    ] {
        l.set(option, value).unwrap();
        assert_eq!(l.get(option).as_deref(), Some(value), "{option}");
    }
    assert!(l.set("border", "3").is_err());
    assert!(l.set("style", "fancy").is_err());
    assert!(l.set("colour", "red").is_err());
    assert_eq!(l.get("colour"), None);
    l.set("pager", "off").unwrap();
    assert_eq!(l.paging, Paging::Off);
    l.set("expanded", "on").unwrap();
    assert_eq!(l.expanded, Expanded::On);
}
