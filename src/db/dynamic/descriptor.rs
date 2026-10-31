//! [ISO/IEC 9075:1992 §17.1] SQL descriptor areas and their item
//! descriptor areas: the fields of Table 17, the codes of Tables 18 to 20,
//! a data type described in them [§17.9 GR3e, GR4e], and the data type
//! their fields describe, if valid \[SR3\], and whether a value's or a
//! target's type matches it \[SR2\].
//!
//! A field never set reads as 0, or as the empty string, and DATA as the
//! null value: what an undefined value reads as is
//! implementation-dependent [§17.2 GR2].

use std::collections::BTreeMap;

use crate::ast::DescriptorItem;
use crate::parser::descriptor_type;
use crate::system::INFORMATION_SCHEMA;
use crate::value::datetime::Field;
use crate::value::{DataType, Decimal, Spelled, Value, spelling};

/// [ISO/IEC 9075:1992 §17.1 Table 18] The codes of the data types.
pub(super) mod code {
    /// CHARACTER.
    pub(in crate::db::dynamic) const CHARACTER: i64 = 1;
    /// NUMERIC.
    pub(in crate::db::dynamic) const NUMERIC: i64 = 2;
    /// DECIMAL.
    pub(in crate::db::dynamic) const DECIMAL: i64 = 3;
    /// INTEGER.
    pub(in crate::db::dynamic) const INTEGER: i64 = 4;
    /// SMALLINT.
    pub(in crate::db::dynamic) const SMALLINT: i64 = 5;
    /// FLOAT.
    pub(in crate::db::dynamic) const FLOAT: i64 = 6;
    /// REAL.
    pub(in crate::db::dynamic) const REAL: i64 = 7;
    /// DOUBLE PRECISION.
    pub(in crate::db::dynamic) const DOUBLE_PRECISION: i64 = 8;
    /// DATE, TIME or TIMESTAMP.
    pub(in crate::db::dynamic) const DATETIME: i64 = 9;
    /// INTERVAL.
    pub(in crate::db::dynamic) const INTERVAL: i64 = 10;
    /// CHARACTER VARYING.
    pub(in crate::db::dynamic) const VARYING: i64 = 12;
    /// BIT.
    pub(in crate::db::dynamic) const BIT: i64 = 14;
    /// BIT VARYING.
    pub(in crate::db::dynamic) const BIT_VARYING: i64 = 15;
    /// \[DESIGN\] An implementation-defined data type, of a code less than 0:
    /// a plug-in's (RULES.md D53), each described by this one code.
    pub(in crate::db::dynamic) const EXTENSION: i64 = -1;
}

/// [ISO/IEC 9075:1992 §17.1 Table 19] The codes of the datetime types:
/// DATE, TIME, TIMESTAMP, TIME WITH TIME ZONE, TIMESTAMP WITH TIME ZONE.
const DATETIME_CODES: [(i64, &str); 5] =
    [(1, "DATE"), (2, "TIME({p})"), (3, "TIMESTAMP({p})"), (4, "TIME({p}) WITH TIME ZONE"), (5, "TIMESTAMP({p}) WITH TIME ZONE")];

/// [ISO/IEC 9075:1992 §17.1 Table 20] The codes of the `<interval
/// qualifier>`s.
const INTERVAL_CODES: [(i64, Field, Field); 13] = [
    (1, Field::Year, Field::Year),
    (2, Field::Month, Field::Month),
    (3, Field::Day, Field::Day),
    (4, Field::Hour, Field::Hour),
    (5, Field::Minute, Field::Minute),
    (6, Field::Second, Field::Second),
    (7, Field::Year, Field::Month),
    (8, Field::Day, Field::Hour),
    (9, Field::Day, Field::Minute),
    (10, Field::Day, Field::Second),
    (11, Field::Hour, Field::Minute),
    (12, Field::Hour, Field::Second),
    (13, Field::Minute, Field::Second),
];

/// [ISO/IEC 9075:1992 §6.1 SR27] The length in positions of a DATE, a
/// TIME, a TIME WITH TIME ZONE, a TIMESTAMP and a TIMESTAMP WITH TIME
/// ZONE, without a fraction of a second.
const DATE_POSITIONS: i64 = 10;
const TIME_POSITIONS: i64 = 8;
const TIME_ZONED_POSITIONS: i64 = 14;
const TIMESTAMP_POSITIONS: i64 = 19;
const TIMESTAMP_ZONED_POSITIONS: i64 = 25;

/// [ISO/IEC 9075:1992 §10.4 SR3, §21.3.23 Description 5] The one character
/// set, SQL_TEXT, of INFORMATION_SCHEMA, and its one collation, of its
/// name: a character string's [§17.9 GR3e iv 1], and the SQL-session's
/// default [§17.5 GR5a].
pub(super) const SQL_TEXT: &str = "SQL_TEXT";

/// The fields of a character set's name, and of a collation's.
const CHARACTER_SET: [DescriptorItem; 3] =
    [DescriptorItem::CharacterSetCatalog, DescriptorItem::CharacterSetSchema, DescriptorItem::CharacterSetName];
const COLLATION: [DescriptorItem; 3] = [DescriptorItem::CollationCatalog, DescriptorItem::CollationSchema, DescriptorItem::CollationName];

/// [ISO/IEC 9075:1992 §17.1] An item descriptor area: the fields set.
#[derive(Debug, Clone, Default)]
pub(super) struct Item {
    fields: BTreeMap<DescriptorItem, Value>,
    /// The type described, of a TYPE less than 0: a plug-in's, which no
    /// field gives (RULES.md D53).
    extension: Option<DataType>,
}

/// The `i64` `n` as an exact value of scale 0.
pub(super) fn exact(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The value of the `usize` `n`, at most `i64::MAX`.
fn number(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Item {
    /// The value of field `f`.
    pub(super) fn get(&self, f: DescriptorItem) -> Value {
        if let Some(v) = self.fields.get(&f) {
            return v.clone();
        }
        if f == DescriptorItem::Data {
            Value::Null
        } else if is_text(f) {
            Value::Char(String::new())
        } else {
            exact(0)
        }
    }

    /// The value of the numeric field `f`.
    pub(super) fn number(&self, f: DescriptorItem) -> i64 {
        match self.fields.get(&f) {
            Some(Value::Exact(d)) => i64::try_from(d.units).unwrap_or(i64::MIN),
            Some(_) | None => 0,
        }
    }

    /// The value of the character string field `f`.
    fn text(&self, f: DescriptorItem) -> String {
        match self.fields.get(&f) {
            Some(Value::Char(s)) => s.clone(),
            Some(_) | None => String::new(),
        }
    }

    /// Sets field `f` to `v`.
    pub(super) fn set(&mut self, f: DescriptorItem, v: Value) {
        self.fields.insert(f, v);
    }

    /// Sets the numeric field `f` to `n`.
    pub(super) fn set_number(&mut self, f: DescriptorItem, n: i64) {
        self.set(f, exact(n));
    }

    /// Field `f` made undefined.
    pub(super) fn forget(&mut self, f: DescriptorItem) {
        self.fields.remove(&f);
    }

    /// Every field but `kept` made undefined, the type described too.
    pub(super) fn keep_only(&mut self, kept: &[DescriptorItem]) {
        self.fields.retain(|f, _| kept.contains(f));
        self.extension = None;
    }

    /// The `<character set name>` of SQL_TEXT in the fields `fields`.
    fn sql_text(&mut self, fields: [DescriptorItem; 3], catalog: &str) {
        for (f, part) in fields.into_iter().zip([catalog, INFORMATION_SCHEMA, SQL_TEXT]) {
            self.set(f, Value::Char(part.to_owned()));
        }
    }

    /// [ISO/IEC 9075:1992 §17.5 GR5a] The SQL-session's default character
    /// set, SQL_TEXT, in its CHARACTER_SET fields.
    pub(super) fn default_character_set(&mut self, catalog: &str) {
        self.sql_text(CHARACTER_SET, catalog);
    }

    /// [ISO/IEC 9075:1992 §17.9 GR3e, GR4e] The descriptor of a column or a
    /// `<dynamic parameter specification>` of type `t`, named `name` if it
    /// has a name not implementation-dependent, NULLABLE as `nullable`, of
    /// the catalog `catalog`. NAME of one unnamed is the empty string.
    pub(super) fn described(t: DataType, name: Option<&str>, nullable: bool, catalog: &str) -> Item {
        let mut item = Item::default();
        item.set_number(DescriptorItem::Type, type_code(t));
        item.set_number(DescriptorItem::Nullable, i64::from(nullable));
        item.set(DescriptorItem::Name, Value::Char(name.unwrap_or_default().to_owned()));
        item.set_number(DescriptorItem::Unnamed, i64::from(name.is_none()));
        item.describe_type(t, catalog);
        item
    }

    /// [ISO/IEC 9075:1992 §17.9 GR3e iv, GR4e iv] The fields of `t` that its
    /// TYPE says.
    fn describe_type(&mut self, t: DataType, catalog: &str) {
        let octets = t.kind().descriptor(&t).octets.map(number);
        match t {
            DataType::Character { length } | DataType::Varying { length } => {
                self.set_number(DescriptorItem::Length, i64::from(length));
                self.set_number(DescriptorItem::OctetLength, octets.unwrap_or(0));
                self.sql_text(CHARACTER_SET, catalog);
                self.sql_text(COLLATION, catalog);
            }
            DataType::Bit { length } | DataType::BitVarying { length } => {
                self.set_number(DescriptorItem::Length, i64::from(length));
                self.set_number(DescriptorItem::OctetLength, octets.unwrap_or(0));
            }
            DataType::Exact { precision, scale, .. } => {
                self.set_number(DescriptorItem::Precision, i64::from(precision));
                self.set_number(DescriptorItem::Scale, i64::from(scale));
            }
            DataType::Approximate { precision, .. } => self.set_number(DescriptorItem::Precision, i64::from(precision)),
            DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. } => {
                let (code, positions, precision) = datetime_shape(t);
                self.set_number(DescriptorItem::Length, positions);
                self.set_number(DescriptorItem::DatetimeIntervalCode, code);
                self.set_number(DescriptorItem::Precision, precision);
            }
            DataType::Interval(q) => {
                let code = INTERVAL_CODES.iter().find(|(_, s, e)| (*s, *e) == (q.start, q.end)).map_or(0, |(c, _, _)| *c);
                self.set_number(DescriptorItem::DatetimeIntervalCode, code);
                self.set_number(DescriptorItem::DatetimeIntervalPrecision, i64::from(q.leading));
                self.set_number(DescriptorItem::Precision, i64::from(q.fraction));
            }
            DataType::Extension(_) => self.extension = Some(t),
        }
    }

    /// [ISO/IEC 9075:1992 §17.1 SR3] The data type its fields describe, if
    /// it is valid: TYPE a code of Table 18, and the fields its type takes
    /// valid for it, a character string's of SQL_TEXT, of the catalog
    /// `catalog`.
    pub(super) fn data_type(&self, catalog: &str) -> Option<DataType> {
        let n = |f| self.number(f);
        let (length, precision, scale) = (n(DescriptorItem::Length), n(DescriptorItem::Precision), n(DescriptorItem::Scale));
        let text = match n(DescriptorItem::Type) {
            code::CHARACTER | code::VARYING => {
                let set: Vec<String> = CHARACTER_SET.iter().map(|f| self.text(*f)).collect();
                if set != [catalog, INFORMATION_SCHEMA, SQL_TEXT] {
                    return None;
                }
                let varying = if n(DescriptorItem::Type) == code::VARYING { " VARYING" } else { "" };
                format!("CHARACTER{varying}({length})")
            }
            code::NUMERIC => format!("NUMERIC({precision}, {scale})"),
            code::DECIMAL => format!("DECIMAL({precision}, {scale})"),
            code::INTEGER => "INTEGER".to_owned(),
            code::SMALLINT => "SMALLINT".to_owned(),
            code::FLOAT => format!("FLOAT({precision})"),
            code::REAL => "REAL".to_owned(),
            code::DOUBLE_PRECISION => "DOUBLE PRECISION".to_owned(),
            code::BIT => format!("BIT({length})"),
            code::BIT_VARYING => format!("BIT VARYING({length})"),
            code::DATETIME => {
                let (_, written) = DATETIME_CODES.iter().find(|(c, _)| *c == n(DescriptorItem::DatetimeIntervalCode))?;
                written.replace("{p}", &precision.to_string())
            }
            code::INTERVAL => self.interval_text()?,
            code::EXTENSION => return self.extension,
            _ => return None,
        };
        descriptor_type(&text)
    }

    /// [ISO/IEC 9075:1992 §17.1 SR3h] The `<interval type>` its
    /// DATETIME_INTERVAL_CODE, DATETIME_INTERVAL_PRECISION and PRECISION
    /// write.
    fn interval_text(&self) -> Option<String> {
        let (_, start, end) = INTERVAL_CODES.iter().find(|(c, _, _)| *c == self.number(DescriptorItem::DatetimeIntervalCode))?;
        let (leading, fraction) = (self.number(DescriptorItem::DatetimeIntervalPrecision), self.number(DescriptorItem::Precision));
        Some(match (start == end, *end == Field::Second) {
            (true, true) => format!("INTERVAL SECOND({leading}, {fraction})"),
            (true, false) => format!("INTERVAL {}({leading})", field_name(*start)),
            (false, true) => format!("INTERVAL {}({leading}) TO SECOND({fraction})", field_name(*start)),
            (false, false) => format!("INTERVAL {}({leading}) TO {}", field_name(*start), field_name(*end)),
        })
    }
}

/// The name of the `<datetime field>` `f`.
fn field_name(f: Field) -> &'static str {
    match f {
        Field::Year => "YEAR",
        Field::Month => "MONTH",
        Field::Day => "DAY",
        Field::Hour => "HOUR",
        Field::Minute => "MINUTE",
        Field::Second => "SECOND",
    }
}

/// Whether `f` is a field of a character string [§17.1 Table 17].
pub(super) fn is_text(f: DescriptorItem) -> bool {
    matches!(f, DescriptorItem::Name) || CHARACTER_SET.contains(&f) || COLLATION.contains(&f)
}

/// [ISO/IEC 9075:1992 §17.1 Table 18] The code of `t`.
fn type_code(t: DataType) -> i64 {
    match t {
        DataType::Character { .. } => code::CHARACTER,
        DataType::Varying { .. } => code::VARYING,
        DataType::Exact { .. } => match spelling(&t) {
            Spelled::Decimal => code::DECIMAL,
            Spelled::Integer => code::INTEGER,
            Spelled::Smallint => code::SMALLINT,
            Spelled::Numeric | Spelled::Float | Spelled::Real | Spelled::DoublePrecision => code::NUMERIC,
        },
        DataType::Approximate { .. } => match spelling(&t) {
            Spelled::Real => code::REAL,
            Spelled::DoublePrecision => code::DOUBLE_PRECISION,
            Spelled::Float | Spelled::Numeric | Spelled::Decimal | Spelled::Integer | Spelled::Smallint => code::FLOAT,
        },
        DataType::Bit { .. } => code::BIT,
        DataType::BitVarying { .. } => code::BIT_VARYING,
        DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. } => code::DATETIME,
        DataType::Interval(_) => code::INTERVAL,
        DataType::Extension(_) => code::EXTENSION,
    }
}

/// [ISO/IEC 9075:1992 §17.9 GR3e iv 5, Table 19, §6.1 SR27] A datetime
/// type's code, its length in positions, and its precision.
fn datetime_shape(t: DataType) -> (i64, i64, i64) {
    let with_fraction = |base: i64, p: u32| {
        let p = i64::from(p);
        base.saturating_add(if p > 0 { p.saturating_add(1) } else { 0 })
    };
    match t {
        DataType::Time { precision, zoned: false } => (2, with_fraction(TIME_POSITIONS, precision), i64::from(precision)),
        DataType::Timestamp { precision, zoned: false } => (3, with_fraction(TIMESTAMP_POSITIONS, precision), i64::from(precision)),
        DataType::Time { precision, zoned: true } => (4, with_fraction(TIME_ZONED_POSITIONS, precision), i64::from(precision)),
        DataType::Timestamp { precision, zoned: true } => (5, with_fraction(TIMESTAMP_ZONED_POSITIONS, precision), i64::from(precision)),
        DataType::Date
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => (1, DATE_POSITIONS, 0),
    }
}

/// [ISO/IEC 9075:1992 §17.1 SR2] Whether a value's type `t`, or a
/// target's if `target`, matches the type `described`: a number of the
/// same name, precision and scale; a fixed-length string of the same
/// length; a varying one of the same, or a target of no shorter.
/// \[DESIGN\] SR2 lists no datetime, interval or plug-in's type: one matches
/// the type it is.
pub(super) fn matches(described: DataType, t: DataType, target: bool) -> bool {
    let named = std::mem::discriminant(&spelling(&described)) == std::mem::discriminant(&spelling(&t));
    match (described, t) {
        (DataType::Exact { precision, scale, .. }, DataType::Exact { precision: its_precision, scale: its_scale, .. }) => {
            named && (matches!(spelling(&t), Spelled::Integer | Spelled::Smallint) || (precision, scale) == (its_precision, its_scale))
        }
        (DataType::Approximate { precision, .. }, DataType::Approximate { precision: its_precision, .. }) => {
            named && (!matches!(spelling(&t), Spelled::Float) || precision == its_precision)
        }
        (DataType::Character { length }, DataType::Character { length: its_length })
        | (DataType::Bit { length }, DataType::Bit { length: its_length }) => length == its_length,
        (DataType::Varying { length }, DataType::Varying { length: its_length })
        | (DataType::BitVarying { length }, DataType::BitVarying { length: its_length }) => {
            its_length == length || (target && its_length > length)
        }
        (_, _) => described == t,
    }
}

/// [ISO/IEC 9075:1992 §17.1] An SQL descriptor area: its occurrences of
/// item descriptor areas [§17.2 GR2], COUNT, and the items set.
#[derive(Debug, Clone)]
pub(crate) struct Descriptor {
    /// The `<occurrences>` it was allocated with.
    pub(super) occurrences: i64,
    /// COUNT [§17.4 GR7a, §17.5 GR3a].
    pub(super) count: i64,
    /// Its item descriptor areas set, by number, from 1.
    pub(super) items: BTreeMap<i64, Item>,
}

impl Descriptor {
    /// A descriptor area of `occurrences` items, each undefined.
    pub(super) fn new(occurrences: i64) -> Descriptor {
        Descriptor { occurrences, count: 0, items: BTreeMap::new() }
    }

    /// Its `n`-th item descriptor area.
    pub(super) fn item(&self, n: i64) -> Item {
        self.items.get(&n).cloned().unwrap_or_default()
    }

    /// [ISO/IEC 9075:1992 §17.9 GR3c-e, GR4c-e] Described: COUNT the number
    /// of `items`, and they the first item descriptor areas, unless there
    /// are more than its occurrences; then whether there were more.
    pub(super) fn describe(&mut self, items: Vec<Item>) -> bool {
        let d = number(items.len());
        self.count = d;
        if d > self.occurrences {
            return true;
        }
        for (n, item) in (1..).zip(items) {
            self.items.insert(n, item);
        }
        false
    }
}
