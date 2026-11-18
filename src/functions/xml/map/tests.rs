//! Tests of XMLMAP's functions: a table's rows written, the data and the
//! XML Schema document of a mapping, and their limits.

use super::*;
use crate::error::Code;
use std::sync::Arc;

use crate::functions::AggregateFunction;
use crate::value::Decimal;
use crate::value::lob::Lob;
use crate::xml::MAX_INPUT;

/// Table A.T of an INTEGER K NOT NULL and a VARCHAR(5) "s t".
fn table() -> Table {
    let column =
        |name: &str, data_type: DataType, nullable| Column { name: name.to_owned(), declared: Declared::Type(data_type), nullable };
    Table {
        schema: "A".to_owned(),
        name: "T".to_owned(),
        columns: vec![
            column("K", DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer }, false),
            column("s t", DataType::varying(5), true),
        ],
    }
}

/// The choices of NULLS `nulls`, AS FOREST if `forest`, of DATA alone.
fn choices(nulls: Nulls, forest: bool, target: &str) -> Choices {
    Choices { nulls, forest, target: target.to_owned(), data: true, metadata: false, encoding: BinaryEncoding::Hex }
}

/// The text of a large object `v`.
fn text_of(v: &Value) -> String {
    let Value::Lob(lob) = v else { panic!("{v} is no text") };
    String::from_utf8(lob.bounded(u64::MAX).unwrap().unwrap()).unwrap()
}

/// The text of the rows `rows`, each written as `shape` says.
fn written(shape: RowShape, rows: &[Vec<Value>]) -> Result<String> {
    let row = XmlMapRow(Arc::new(shape));
    rows.iter().map(|r| row.call(r).map(|v| text_of(&v))).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.10 GR6] a table's
/// rows: a `row` of each, mapped as an element, of an element of each
/// value, its name escaped, a null value's left out or nilled; as a
/// forest, an element of the table's name, declaring `xsi` of NULLS nil
/// and the target namespace of a table mapped alone.
#[test]
fn rows_written_9_10() {
    let rows = [vec![Value::Exact(Decimal::new(1, 0)), Value::Char("a&b".to_owned())], vec![Value::Exact(Decimal::new(2, 0)), Value::Null]];
    let absent = written(RowShape::of(&table(), &choices(Nulls::Absent, false, ""), true), &rows).unwrap();
    assert_eq!(absent, "<row><K>1</K><s_x0020_t>a&amp;b</s_x0020_t></row><row><K>2</K></row>");
    let nil = written(RowShape::of(&table(), &choices(Nulls::Nil, true, "urn:t"), true), &rows).unwrap();
    let start = r#"<T xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns="urn:t">"#;
    assert_eq!(nil, format!(r#"{start}<K>1</K><s_x0020_t>a&amp;b</s_x0020_t></T>{start}<K>2</K><s_x0020_t xsi:nil="true"/></T>"#));
    let within = written(RowShape::of(&table(), &choices(Nulls::Absent, true, "urn:t"), false), &rows[1..]).unwrap();
    assert_eq!(within, "<T><K>2</K></T>", "a table of a schema of no target namespace");
}

/// The text the set function of `shape` writes of `rows`, each its
/// table's index and its text.
fn data(shape: DataShape, rows: &[(i128, &str)]) -> Result<String> {
    let mut acc = XmlMapRows(Arc::new(shape)).accumulator(rows_type());
    for (index, text) in rows {
        acc.update(&Value::Row(vec![Value::Exact(Decimal::new(*index, 0)), Value::Lob(Lob::of_text(text))].into()))?;
    }
    Ok(text_of(&acc.finish()?))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.11 GR12c, §9.13
/// GR6, §9.16 GR6] the data written as rows come: each table's rows in
/// its element, a table of no row its element empty, each schema's in
/// its element, the catalog's root declaring `xsi`; of a table as a
/// forest, its rows alone.
#[test]
fn data_written_9_16() {
    let (a, b) = (Table { name: "U".to_owned(), ..table() }, Table { schema: "B".to_owned(), ..table() });
    let catalog = Source::Catalog(vec![("A".to_owned(), vec![table(), a]), ("B".to_owned(), vec![b])]);
    let shape = DataShape::of(&catalog, &choices(Nulls::Absent, false, "urn:c"), "C");
    let text = data(shape, &[(0, "<row/>"), (0, "<row/>"), (2, "<row>b</row>")]).unwrap();
    let root = r#"<C xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns="urn:c">"#;
    assert_eq!(text, format!("{root}<A><T><row/><row/></T><U></U></A><B><T><row>b</row></T></B></C>"));
    let forest = DataShape::of(&Source::Table(table()), &choices(Nulls::Nil, true, ""), "C");
    assert_eq!(data(forest, &[(0, "<T/>"), (0, "<T/>")]).unwrap(), "<T/><T/>");
    let none = DataShape::of(&Source::Schema("A".to_owned(), vec![table()]), &choices(Nulls::Absent, false, ""), "C");
    assert_eq!(
        DataWriter::new(&none).unwrap().finish(&none).unwrap(),
        r#"<A xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><T></T></A>"#
    );
    let schema = Source::Schema("A".to_owned(), vec![table(), table()]);
    let backwards = data(DataShape::of(&schema, &choices(Nulls::Absent, false, ""), "C"), &[(1, "<row/>"), (0, "<row/>")]);
    assert_eq!(backwards.unwrap_err().code, Code::Corrupt, "a row of a table before the last begun");
    let mut acc = XmlMapRows(Arc::new(DataShape::of(&schema, &choices(Nulls::Absent, false, ""), "C"))).accumulator(rows_type());
    assert_eq!(acc.update(&Value::Null).unwrap_err().code, Code::Api, "a value no row");
}

/// Verifies: [DESIGN] RULES.md D18, D86 a mapping's text held whole, of
/// all its tables, is bounded as it grows: past the limit, SQLCODE -900,
/// as the row that takes it there comes, whichever table it is of; a
/// row's text past `MAX_INPUT` octets too.
#[test]
fn texts_past_the_limit() {
    let schema = Source::Schema("A".to_owned(), vec![table(), table(), table()]);
    let limited = |limit| DataShape::of(&schema, &choices(Nulls::Absent, false, ""), "C").limited(limit);
    let rows = [(0, "<row>0123456789</row>"), (1, "<row>0123456789</row>")];
    let whole = data(limited(MAX_INPUT), &rows).unwrap().len();
    assert_eq!(data(limited(whole), &rows).unwrap().len(), whole, "within the limit");
    assert_eq!(data(limited(whole.saturating_sub(1)), &rows).unwrap_err().code, Code::Unsupported, "past it as the data ends");
    let first = data(limited(MAX_INPUT), &rows[..1]).unwrap().len();
    let past = data(limited(first), &[rows[0], rows[1], (2, "<row/>")]);
    assert_eq!(past.unwrap_err().code, Code::Unsupported, "past it as the second table's row comes");
    let shape = RowShape::of(&table(), &choices(Nulls::Absent, false, ""), true);
    let long = vec![Value::Exact(Decimal::new(1, 0)), Value::Char("x".repeat(MAX_INPUT))];
    assert_eq!(written(shape, &[long]).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] RULES.md D86 the type of a mapping's value: of
/// METADATA, or DATA of an element, XML(DOCUMENT(UNTYPED)); of DATA of a
/// table as a forest, XML(CONTENT(UNTYPED)), as of a schema's it is not;
/// of both, XML(SEQUENCE).
#[test]
fn types_of_mappings() {
    let of = |source: Source, (data, metadata, forest): (bool, bool, bool)| {
        let choices = Choices { data, metadata, ..choices(Nulls::Absent, forest, "") };
        XmlMap { catalog: "C".to_owned(), source, choices }.result_type()
    };
    let document = DataType::Xml(XmlType::Document(Typing::Untyped));
    assert_eq!(of(Source::Table(table()), (true, false, false)), document);
    assert_eq!(of(Source::Table(table()), (false, true, true)), document);
    assert_eq!(of(Source::Table(table()), (true, false, true)), DataType::Xml(XmlType::Content(Typing::Untyped)));
    assert_eq!(of(Source::Schema("A".to_owned(), Vec::new()), (true, false, true)), document);
    assert_eq!(of(Source::Catalog(Vec::new()), (true, true, false)), DataType::Xml(XmlType::Sequence));
}
