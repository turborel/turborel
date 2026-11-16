//! [ISO/IEC 9075-2:2023 §4.35.4, §9.24, §20.6] The PTF descriptor areas of
//! a PTF data area: SQL descriptor areas made as the compilation of a
//! polymorphic table function's invocation makes them, read for its
//! result row type, and changed by the statements of its component
//! procedures that SQL descriptor areas of a session have none of, COPY
//! DESCRIPTOR and FETCH from a PTF cursor; GET and SET DESCRIPTOR are
//! `areas.rs`'s, of either.

use std::sync::Arc;

use super::descriptor::{Descriptor, Item, code, exact, number};
use super::dynamic_error;
use super::nested::described;
use crate::ast::{CopyOptions, DescriptorItem};
use crate::error::{Result, SqlError, SqlState};
use crate::value::types::MAX_TYPE_NESTING;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §20.6 GR7h iii 2] The components of an item that
/// describe its type, which COPY DESCRIPTOR with TYPE copies: of those an
/// item has, which has no SCOPE components, as there are no reference
/// types.
const TYPE_FIELDS: [DescriptorItem; 15] = [
    DescriptorItem::Type,
    DescriptorItem::Length,
    DescriptorItem::Precision,
    DescriptorItem::Scale,
    DescriptorItem::DatetimeIntervalCode,
    DescriptorItem::DatetimeIntervalPrecision,
    DescriptorItem::CharacterSetCatalog,
    DescriptorItem::CharacterSetSchema,
    DescriptorItem::CharacterSetName,
    DescriptorItem::CollationCatalog,
    DescriptorItem::CollationSchema,
    DescriptorItem::CollationName,
    DescriptorItem::UserDefinedTypeCatalog,
    DescriptorItem::UserDefinedTypeSchema,
    DescriptorItem::UserDefinedTypeName,
];

/// [ISO/IEC 9075-2:2023 §9.24 GR2d ii 3, GR2e iii] The most item descriptor
/// areas the initial and the intermediate result row type descriptor areas
/// have when no `<table function column list>` fixes them (IL056, IL057).
pub(crate) const MAX_RESULT_ITEMS: i64 = 1_000;

/// [ISO/IEC 9075-2:2023 §9.24 GR2d iii 1, GR2d iv 1] A SORT_DIRECTION or
/// NULL_ORDERING of ASC or NULLS FIRST.
const FIRST: i64 = 1;
/// One of DESC or NULLS LAST.
const LAST: i64 = -1;

/// An exception of dynamic SQL, invalid descriptor index (07009), of the
/// General Rule `rule`: `msg`.
fn bad_index(rule: &str, msg: &str) -> SqlError {
    dynamic_error(SqlState::INVALID_DESCRIPTOR_INDEX, format!("[ISO/IEC 9075-2:2023 {rule}] {msg}"))
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b iv, v, GR2d ii, GR2e] A PTF descriptor
/// area of `occurrences` item descriptor areas, none set, COUNT and
/// TOP_LEVEL_COUNT 0.
pub(crate) fn empty(occurrences: usize) -> Descriptor {
    Descriptor::new(number(occurrences))
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b ii, GR2d i; §20.10 GR6, GR7] A PTF
/// descriptor area that describes the row of `columns`, each its name and
/// type, its fields' items subordinate to it; of as many item descriptor
/// areas as it needs and `more` more, of the catalog `catalog`.
pub(crate) fn of_row(columns: &[(String, DataType)], more: usize, catalog: &str) -> Descriptor {
    let mut items = Vec::new();
    for (name, t) in columns {
        let top = Item::described(t.clone(), Some(name), true, catalog);
        items.extend(described(t, top, true, catalog));
    }
    let mut area = Descriptor::new(number(items.len().saturating_add(more)));
    area.describe(items, (columns.len(), 0));
    area
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b iii 1] A PTF descriptor area of the
/// partitioning of a table argument: an item of LEVEL 0 named for each
/// partitioning column, its other components undefined.
pub(crate) fn of_names(names: &[String]) -> Descriptor {
    let mut area = Descriptor::new(number(names.len()));
    let items = names.iter().map(|n| top_named(n)).collect();
    area.describe(items, (names.len(), 0));
    area
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b iii 2] A PTF descriptor area of the
/// ordering of a table argument: an item of LEVEL 0 for each ordering
/// column, its name, whether DESC and whether NULLS FIRST, SORT_DIRECTION
/// and NULL_ORDERING 1 or −1.
pub(crate) fn of_ordering(columns: &[(String, bool, bool)]) -> Descriptor {
    let mut area = Descriptor::new(number(columns.len()));
    let items = columns
        .iter()
        .map(|(name, descending, nulls_first)| {
            let mut item = top_named(name);
            item.set(DescriptorItem::SortDirection, exact(if *descending { LAST } else { FIRST }));
            item.set(DescriptorItem::NullOrdering, exact(if *nulls_first { FIRST } else { LAST }));
            item
        })
        .collect();
    area.describe(items, (columns.len(), 0));
    area
}

/// An item of LEVEL 0 named `name`, its other components undefined.
fn top_named(name: &str) -> Item {
    let mut item = Item::default();
    item.set(DescriptorItem::Level, exact(0));
    item.set(DescriptorItem::Name, Value::Char(name.to_owned()));
    item
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12k ii, iii] The NAME of each of the first
/// COUNT items of `area`, if each is of LEVEL 0: else none.
pub(crate) fn top_names(area: &Descriptor) -> Option<Vec<String>> {
    (1..=area.count).map(|n| area.item(n)).map(|i| (i.level() == 0).then(|| i.text(DescriptorItem::Name))).collect()
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12l iii, SR12m] The columns `area`
/// describes: each item of LEVEL 0 among its first COUNT, its NAME and
/// the declared type it matches with its subordinate items; none if one
/// matches no declared type.
pub(crate) fn columns(area: &Descriptor, catalog: &str) -> Option<Vec<(String, DataType)>> {
    let tops = area.tops(catalog)?;
    Some(tops.into_iter().map(|(n, t)| (area.item(n).text(DescriptorItem::Name), t)).collect())
}

/// [ISO/IEC 9075-2:2023 §20.28 GR11] The value of each column `area`
/// describes, of the types `types` it describes: null if its INDICATOR is
/// negative; of a row, its fields' items' values; else DATA. \[DESIGN\] A
/// DATA left undefined gives the null value.
pub(crate) fn data(area: &Descriptor, types: &[DataType], catalog: &str) -> Vec<Value> {
    let mut out = Vec::with_capacity(types.len());
    let mut n = 1_i64;
    for t in types {
        out.push(value_at(area, n, t, (catalog, 0)));
        n = area.represented(n, area.count, catalog).map_or(n.saturating_add(1), |(_, next)| next);
    }
    out
}

/// The value of item `n` of `area`, of type `t`, as `data` takes it, at
/// `depth`: types nest at most `MAX_TYPE_NESTING` deep, so this does too
/// (RULES.md R-CF-6).
fn value_at(area: &Descriptor, n: i64, t: &DataType, (catalog, depth): (&str, usize)) -> Value {
    let item = area.item(n);
    if item.number(DescriptorItem::Indicator) < 0 || depth >= MAX_TYPE_NESTING {
        return Value::Null;
    }
    let DataType::Row(r) = t else { return item.get(DescriptorItem::Data) };
    if item.code() != code::ROW {
        return item.get(DescriptorItem::Data);
    }
    let mut values = Vec::with_capacity(r.fields.len());
    let mut k = n.saturating_add(1);
    for f in &r.fields {
        values.push(value_at(area, k, &f.data_type, (catalog, depth.saturating_add(1))));
        k = area.represented(k, area.count, catalog).map_or(k.saturating_add(1), |(_, next)| next);
    }
    Value::Row(Arc::from(values))
}

/// [ISO/IEC 9075-2:2023 §20.12 GR5e i] The row `row`, of the types `types`,
/// put in the items of LEVEL 0 of `area`, one for each, as FETCH INTO a
/// PTF descriptor area puts a cursor's row: each item set to reflect its
/// value, and its subordinate items too.
pub(crate) fn put_row(area: &mut Descriptor, row: &[Value], types: &[DataType], catalog: &str) {
    let mut n = 1_i64;
    for (v, t) in row.iter().zip(types) {
        let next = area.represented(n, area.count, catalog).map_or(n.saturating_add(1), |(_, after)| after);
        area.reflect(n, (t, catalog), v, (v, t));
        n = next;
    }
}

/// [ISO/IEC 9075-2:2023 §20.6 GR6] COPY of the whole of `source` to
/// `target`.
///
/// # Errors
///
/// SQLSTATE 07009 if `source` has more item descriptor areas than `target`
/// may \[GR6a\].
pub(crate) fn copy_whole(source: &Descriptor, target: &mut Descriptor) -> Result<()> {
    if source.count > target.occurrences {
        return Err(bad_index("§20.6 GR6a", "the source has more item descriptor areas than the target may"));
    }
    target.count = source.count;
    target.top_level = source.top_level;
    target.key_type = source.key_type;
    target.items = source.items.clone();
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §20.6 GR7] COPY of item `from` of `source` to item
/// `to` of `target`, of `options`.
///
/// # Errors
///
/// SQLSTATE 07009 if either number is of no item, or of one not of LEVEL
/// 0, or the target may not have the items the copy needs \[GR7b, c, e,
/// f, h iii 2 B I\].
pub(crate) fn copy_item(source: &Descriptor, from: i64, (target, to): (&mut Descriptor, i64), options: CopyOptions) -> Result<()> {
    if !(1..=source.count).contains(&from) || source.item(from).level() != 0 {
        return Err(bad_index("§20.6 GR7b, GR7c", &format!("item {from} of the source is not one of LEVEL 0")));
    }
    if !(1..=target.occurrences).contains(&to) {
        return Err(bad_index("§20.6 GR7e", &format!("item {to} is not of 1 to {}, the target's", target.occurrences)));
    }
    if to <= target.count {
        if target.item(to).level() != 0 {
            return Err(bad_index("§20.6 GR7f i", &format!("item {to} of the target is not of LEVEL 0")));
        }
    } else {
        grow(target, to);
        target.top_level = number((1..=source.count).filter(|n| source.item(*n).level() == 0).count());
    }
    let si = source.item(from);
    let mut ti = target.item(to);
    match options {
        CopyOptions::Data => ti.set(DescriptorItem::Data, si.get(DescriptorItem::Data)),
        CopyOptions::Name => ti.set(DescriptorItem::Name, si.get(DescriptorItem::Name)),
        CopyOptions::Type | CopyOptions::NameType => {
            if options == CopyOptions::NameType {
                ti.set(DescriptorItem::Name, si.get(DescriptorItem::Name));
            }
            target.items.insert(to, ti);
            return copy_type(source, from, (target, to));
        }
    }
    target.items.insert(to, ti);
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §20.6 GR7h iii 2] The type of item `from` of
/// `source` copied to item `to` of `target`: with its subordinate items'
/// LEVEL and type, each to the item as far after `to` as it is after
/// `from`. \[DESIGN\] GR7h iii 2 B III 1 reads the source's items from
/// item 2's number, which is item 1's.
fn copy_type(source: &Descriptor, from: i64, (target, to): (&mut Descriptor, i64)) -> Result<()> {
    let subordinates = (from.saturating_add(1)..=source.count).take_while(|k| source.item(*k).level() > 0).count();
    let k = number(subordinates);
    if to.saturating_add(k) > target.occurrences {
        return Err(bad_index("§20.6 GR7h iii 2 B I", "the target may not have the items the type's subordinate items need"));
    }
    grow(target, to.saturating_add(k));
    for i in 0..=k {
        let si = source.item(from.saturating_add(i));
        let mut ti = target.item(to.saturating_add(i));
        if i > 0 {
            ti.set(DescriptorItem::Level, si.get(DescriptorItem::Level));
        }
        for f in TYPE_FIELDS {
            ti.set(f, si.get(f));
        }
        ti.copy_described(&si);
        target.items.insert(to.saturating_add(i), ti);
    }
    if k > 0 {
        target.top_level = number((1..=target.count).filter(|n| target.item(*n).level() == 0).count());
    }
    Ok(())
}

/// COUNT of `area` made at least `to`, each new item of LEVEL 0
/// [ISO/IEC 9075-2:2023 §20.6 GR7f ii, GR7h iii 2 B II].
fn grow(area: &mut Descriptor, to: i64) {
    for n in area.count.saturating_add(1)..=to {
        let mut item = area.item(n);
        item.set(DescriptorItem::Level, exact(0));
        area.items.insert(n, item);
    }
    area.count = area.count.max(to);
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2c, §20.16] The PTF descriptor area a
/// `<descriptor value constructor>` of `columns` specifies: an item of
/// LEVEL 0 for each, its NAME the column's name and, if its type is
/// written, the components that describe it, with its subordinate items.
pub(crate) fn of_descriptor_columns(columns: &[(String, Option<DataType>)], catalog: &str) -> Descriptor {
    let mut items = Vec::new();
    for (name, t) in columns {
        match t {
            Some(t) => {
                let top = Item::described(t.clone(), Some(name), true, catalog);
                items.extend(described(t, top, true, catalog));
            }
            None => items.push(top_named(name)),
        }
    }
    let mut area = Descriptor::new(number(items.len()));
    area.describe(items, (columns.len(), 0));
    area
}
