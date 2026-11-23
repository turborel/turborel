//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.5] The TIN tables.
//! ST_TINTable() is a table function, `TABLE(ST_TINTABLE(tin)) AS T`, of
//! the ten columns of Description 2a. ST_TINTable(name, columns...,
//! maxsidelength) is a scalar routine of SELF first that reads the table
//! its arguments name: the binder appends the rows of the table, read in
//! the statement's snapshot, as an array of rows (RULES.md D87), so the
//! names are constants. Each name is an identifier or, for the table, a
//! schema and an identifier; a delimited identifier keeps its case.

use super::{number, spatial};
use crate::error::{Result, SqlState};
use crate::functions::builtin::TableFunction;
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::geometry::model::{Geom, Kind};
use crate::geometry::{data_type, data_type_of};
use crate::spatial::solids::table::{Row, from_rows, table};
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::{CharUnits, DOUBLE_PRECISION, DataType, Decimal, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
/// DT1 and DT2 [§8.6.5 DR3, DR4].
const NAME: DataType = DataType::Varying { length: 128, units: CharUnits::Characters };
const NAME_ARGUMENTS: usize = 11;
/// SELF, the eleven names and the maximum side length.
const ARGUMENTS: usize = 13;
const NAME_NOTE: &str = "§8.6.5 Description 4a iv";

const fn varying(length: u32) -> DataType {
    DataType::Varying { length, units: CharUnits::Characters }
}

/// §8.6.5 ST_TINTable(), a table function.
#[derive(Debug, Clone, Copy)]
pub struct TinTable;

impl TableFunction for TinTable {
    fn name(&self) -> &'static str {
        "ST_TINTABLE"
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == 1).then(|| vec![data_type()])
    }

    fn columns(&self) -> Vec<(String, DataType)> {
        let integers = |max| DataType::array(INTEGER, max);
        [
            ("ITEM", varying(30)),
            ("ORDERNUMBER", INTEGER),
            ("XCOORD", DOUBLE),
            ("YCOORD", DOUBLE),
            ("ZCOORD", DOUBLE),
            ("TRIANGLE", integers(3)),
            ("VISIBILITY", INTEGER),
            ("ELEMENTID", INTEGER),
            ("ELEMENTTAG", varying(64)),
            ("ELEMENT", integers(MAX_ARRAY_CARDINALITY)),
        ]
        .into_iter()
        .map(|(n, t)| (n.to_owned(), t))
        .collect()
    }

    fn rows(&self, args: &[Value], _read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let Some(v) = args.first().filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
        Ok(table(&Geom::from_value(v)?)?.into_iter().map(values).collect())
    }
}

fn whole(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn or_null<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Value {
    v.map_or(Value::Null, f)
}

fn integers(all: &[i64]) -> Value {
    Value::Array(all.iter().map(|n| whole(*n)).collect::<Vec<_>>().into())
}

/// A row's values, in the columns' order.
fn values(r: Row) -> Vec<Value> {
    let (x, y, z) = r.point.map_or((None, None, None), |(x, y, z)| (Some(x), Some(y), z));
    vec![
        Value::Char(r.item.into()),
        whole(r.order),
        or_null(x, Value::Approx),
        or_null(y, Value::Approx),
        or_null(z, Value::Approx),
        or_null(r.triangle, |t| integers(&t)),
        or_null(r.visibility, whole),
        or_null(r.id, whole),
        or_null(r.tag, |s| Value::Char(s.into())),
        or_null(r.element, |e| integers(&e)),
    ]
}

/// §8.6.5 ST_TINTable(DT1, DT2 ..., DOUBLE PRECISION), of SELF first.
#[derive(Debug, Clone, Copy)]
pub struct TinFromTable;

fn invalid(detail: &str) -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_INVALID_ARGUMENT, NAME_NOTE, detail)
}

/// The parts of the identifier chain `text`, each delimited, at most
/// `most` of them: a regular identifier folds to upper case.
fn chain(text: &str, most: usize) -> Option<Vec<String>> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.is_empty() || parts.len() > most {
        return None;
    }
    let one = |p: &str| -> Option<String> {
        if let Some(inner) = p.strip_prefix('"').and_then(|q| q.strip_suffix('"')) {
            return (!inner.is_empty() && !inner.contains('"')).then(|| inner.to_owned());
        }
        let mut chars = p.chars();
        let regular = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        regular.then(|| p.to_ascii_uppercase())
    };
    parts.into_iter().map(one).collect()
}

/// The text of a delimited identifier of `name`.
fn delimited(name: &str) -> String {
    format!("\"{name}\"")
}

/// The query of the table and columns the constants name.
fn query(constants: &[Option<&Value>]) -> Option<AppendedQuery> {
    let names: Vec<&str> = constants
        .iter()
        .skip(1)
        .take(NAME_ARGUMENTS)
        .map(|c| if let Some(Value::Char(s)) = c { Some(s.as_str()) } else { None })
        .collect::<Option<_>>()?;
    let (table, columns) = names.split_first()?;
    let table = chain(table, 2)?;
    let columns: Vec<String> = columns.iter().map(|c| chain(c, 1).and_then(|mut p| p.pop())).collect::<Option<_>>()?;
    let (schema, ident) = match table.as_slice() {
        [schema, ident] => (schema.clone(), ident.clone()),
        [ident] => (String::new(), ident.clone()),
        _ => return None,
    };
    let from = table.iter().map(|p| delimited(p)).collect::<Vec<_>>().join(".");
    let fields = columns.iter().map(|c| delimited(c)).collect::<Vec<_>>().join(", ");
    let query = format!("SELECT ROW({fields}) FROM {from}");
    Some(AppendedQuery { query, tables: vec![(schema, ident)], columns })
}

/// The engine's row of a row of the table read.
fn row(v: &Value) -> Result<Option<Row>> {
    let Value::Row(fields) = v else { return Err(invalid("the table's rows are not readable")) };
    let [item, order, x, y, z, triangle, _visibility, id, tag, element] = fields.as_ref() else {
        return Err(invalid("the table has not the ten columns"));
    };
    let (Value::Char(item), Value::Exact(order)) = (item, order) else { return Ok(None) };
    let text = |field: &Value| if let Value::Char(s) = field { Some(s.to_string()) } else { None };
    let opt = |field: &Value| if field.is_null() { Ok(None) } else { number(field).map(Some) };
    let z = opt(z)?;
    let point = opt(x)?.zip(opt(y)?).map(|(x, y)| (x, y, z));
    let triangle = list(triangle).and_then(|t| <[i64; 3]>::try_from(t).ok());
    let order = i64::try_from(order.units).map_err(|_| invalid("an ordernumber is out of range"))?;
    let id = if let Value::Exact(d) = id { i64::try_from(d.units).ok() } else { None };
    Ok(Some(Row { item: item.trim().to_owned(), order, point, triangle, visibility: None, id, tag: text(tag), element: list(element) }))
}

/// The integers of an INTEGER ARRAY value; none of the null value.
fn list(v: &Value) -> Option<Vec<i64>> {
    let Value::Array(items) = v else { return None };
    let one = |i: &Value| if let Value::Exact(d) = i { i64::try_from(d.units).ok() } else { None };
    items.iter().map(one).collect::<Option<Vec<_>>>()
}

impl ScalarFunction for TinFromTable {
    fn name(&self) -> &'static str {
        "ST_TINTABLE"
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fits = args.len() == ARGUMENTS || args.len() == ARGUMENTS.saturating_add(1);
        let tin =
            args.first().and_then(crate::geometry::of_data_type).is_some_and(|k| k.is_subtype_of(Kind::Tin) || Kind::Tin.is_subtype_of(k));
        if !fits || !tin {
            return Err(super::signature("ST_TINTABLE"));
        }
        Ok(data_type_of(Kind::Tin))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        match position {
            0 => known.first().cloned().flatten().or_else(|| Some(data_type_of(Kind::Tin))),
            12 => Some(DOUBLE),
            13.. => None,
            _ => Some(NAME),
        }
    }

    fn appended_query(&self, constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        query(constants)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.get(12).is_some_and(Value::is_null) {
            return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, "§8.6.4 Description 4a", "the maximum side length is null"));
        }
        if args.first().is_some_and(Value::is_null) {
            return Ok(Value::Null);
        }
        let Some(Value::Array(read)) = args.get(ARGUMENTS) else {
            return Err(invalid("the table or a column is not named as an identifier, or does not exist"));
        };
        let rows: Vec<Row> = read.iter().map(row).collect::<Result<Vec<_>>>()?.into_iter().flatten().collect();
        let max_side = number(args.get(12).unwrap_or(&Value::Null))?;
        Ok(from_rows(&rows, max_side)?.to_value())
    }

    fn deterministic(&self) -> bool {
        false
    }
}
