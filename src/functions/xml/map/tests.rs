//! Tests of XMLMAP's functions: a table's rows written, the data and the
//! XML Schema document of a mapping, and their limits.

use super::*;
use crate::error::Code;
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

/// The text the set function of `shape` writes of `rows`.
fn written(shape: RowShape, rows: &[Vec<Value>]) -> Result<String> {
    let mut acc = XmlMapRows(std::sync::Arc::new(shape)).accumulator(rows_type());
    for row in rows {
        acc.update(&Value::Row(row.clone().into()))?;
    }
    let Value::Lob(lob) = acc.finish()? else { return Err(crate::error::SqlError::new(Code::Api, "no text")) };
    let bytes = lob.bounded(u64::MAX)?.unwrap_or_default();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
    assert_eq!(written(RowShape::of(&table(), &choices(Nulls::Absent, false, ""), true), &[]).unwrap(), "");
    let mut acc =
        XmlMapRows(std::sync::Arc::new(RowShape::of(&table(), &choices(Nulls::Absent, false, ""), true))).accumulator(rows_type());
    assert_eq!(acc.update(&Value::Null).unwrap_err().code, Code::Api, "a value no row");
}

/// Verifies: [DESIGN] RULES.md D18, D86 a table's rows written past
/// `MAX_INPUT` octets, and a mapping's data past it, are SQLCODE -900,
/// before more is held.
#[test]
fn texts_past_the_limit() {
    let half = "x".repeat(MAX_INPUT.div_ceil(2));
    let row = vec![Value::Exact(Decimal::new(1, 0)), Value::Char(half.clone())];
    let shape = RowShape::of(&table(), &choices(Nulls::Absent, false, ""), true);
    assert_eq!(written(shape, &[row.clone(), row]).unwrap_err().code, Code::Unsupported);
    let map = XmlMap {
        catalog: "C".to_owned(),
        source: Source::Schema("A".to_owned(), vec![table(), table()]),
        choices: choices(Nulls::Absent, true, ""),
    };
    let rows = [Value::Lob(Lob::of_text(&half)), Value::Lob(Lob::of_text(&half))];
    assert_eq!(map.data_document(&rows).unwrap_err().code, Code::Unsupported);
    assert_eq!(
        map.data_document(&rows[..1]).unwrap().len(),
        half.len().saturating_add("<A xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"></A>".len())
    );
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
