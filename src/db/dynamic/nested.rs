//! [ISO/IEC 9075-2:2023 §20.1 SR3-SR6] The item descriptor areas of a row
//! type's fields and of an array type's element: each subordinate to the
//! one before it of a LEVEL one less, in the order SR5 implies, so an item
//! of TYPE ROW is followed by DEGREE items, each with its own, and one of
//! TYPE ARRAY by one. The type an item represents is its own and theirs
//! [SR8]; the value it gives a dynamic parameter is its DATA, or of a row
//! the values its fields' items give [§20.11 GR6b i 3]; a value put in it
//! goes to them as well [§20.12 GR5e i].

use std::sync::Arc;

use super::descriptor::{Descriptor, Item, code, exact};
use super::dynamic_error;
use crate::ast::DescriptorItem;
use crate::error::{Result, SqlError, SqlState};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY, MAX_TYPE_NESTING};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §20.5 GR4b] A LEVEL that would leave the items
/// out of the order SR5 gives them (SQLSTATE 0700E): `msg`.
fn invalid_level(rule: &str, msg: &str) -> SqlError {
    dynamic_error(SqlState::INVALID_LEVEL_VALUE, format!("[ISO/IEC 9075-2:2023 §20.5 {rule}] {msg}"))
}

/// [ISO/IEC 9075-2:2023 §20.10 GR6f, GR7] The items that describe a value
/// of type `t`: `top`, of LEVEL 0, then, if `nesting`, each subordinate
/// descriptor of `t` in the order SR5 gives them, of the catalog `catalog`.
pub(super) fn described(t: &DataType, top: Item, nesting: bool, catalog: &str) -> Vec<Item> {
    let mut items = vec![top];
    if nesting {
        subordinates(t, 0, catalog, &mut items);
    }
    items
}

/// Adds to `out` the subordinate descriptors of `t`, of an item of LEVEL
/// `level` [§20.1 SR4, SR5]: of each field of a row type, or of an array
/// type's element, an item of LEVEL `level` + 1, NULLABLE 1, then its own
/// [§20.10 GR7a, GR7c iii]. A field's NAME is its name, UNNAMED 1 if the
/// name was made for it. An element's NAME is the empty string, its
/// UNNAMED 1. Types nest at most `MAX_TYPE_NESTING` deep, so `level` does
/// too (RULES.md R-CF-6).
fn subordinates(t: &DataType, level: i64, catalog: &str, out: &mut Vec<Item>) {
    if usize::try_from(level).map_or(true, |l| l >= MAX_TYPE_NESTING) {
        return;
    }
    let below = level.saturating_add(1);
    let add = |part: &DataType, (name, unnamed): (&str, bool), into: &mut Vec<Item>| {
        let mut item = Item::described(part.clone(), Some(name), true, catalog);
        item.set_number(DescriptorItem::Level, below);
        item.set_number(DescriptorItem::Unnamed, i64::from(unnamed));
        into.push(item);
        subordinates(part, below, catalog, into);
    };
    match t {
        DataType::Row(r) => r.fields.iter().for_each(|f| add(&f.data_type, (&f.name, f.unnamed), out)),
        DataType::Array(a) => add(&a.element, ("", true), out),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => {}
    }
}

impl Descriptor {
    /// Whether item `k`, of the first `bound`, is immediately subordinate
    /// to an item of LEVEL `level`, if it follows it before any of LEVEL
    /// `level` or less [§20.1 SR3].
    fn below(&self, k: i64, bound: i64, level: i64) -> bool {
        k <= bound && self.item(k).level() == level.saturating_add(1)
    }

    /// [ISO/IEC 9075-2:2023 §20.1 SR6, SR8] The type item `n`, of the first
    /// `bound`, represents with its subordinate items, if it is valid, and
    /// the number of the item after them: of TYPE ROW, DEGREE 1 or more
    /// and exactly that many immediately subordinate items, each valid
    /// \[SR6p\]; of TYPE ARRAY, a valid CARDINALITY and exactly one \[SR6q\];
    /// else as its own fields give it, of the catalog `catalog`.
    pub(super) fn represented(&self, n: i64, bound: i64, catalog: &str) -> Option<(DataType, i64)> {
        self.represented_at(n, (bound, 0), catalog)
    }

    /// `represented`, `depth` deep in composite types, as deep as they nest
    /// at most (RULES.md R-CF-6).
    fn represented_at(&self, n: i64, (bound, depth): (i64, usize), catalog: &str) -> Option<(DataType, i64)> {
        if n < 1 || n > bound || depth > MAX_TYPE_NESTING {
            return None;
        }
        let item = self.item(n);
        let (level, deeper) = (item.level(), (bound, depth.saturating_add(1)));
        match item.code() {
            code::ROW => {
                let degree = usize::try_from(item.number(DescriptorItem::Degree)).ok().filter(|d| *d >= 1)?;
                let (mut fields, mut next) = (Vec::with_capacity(degree), n.saturating_add(1));
                for i in 1..=degree {
                    if !self.below(next, bound, level) {
                        return None;
                    }
                    let (data_type, after) = self.represented_at(next, deeper, catalog)?;
                    // A field's name has no part in the type's matching [SR7t].
                    let named = self.item(next).text(DescriptorItem::Name);
                    let unnamed = named.is_empty();
                    let name = if unnamed { format!("F{i}") } else { named };
                    fields.push(Field { name, data_type, unnamed });
                    next = after;
                }
                (!self.below(next, bound, level)).then(|| (DataType::row(fields), next))
            }
            code::ARRAY => {
                let max =
                    u32::try_from(item.number(DescriptorItem::Cardinality)).ok().filter(|m| (1..=MAX_ARRAY_CARDINALITY).contains(m))?;
                let first = n.saturating_add(1);
                if !self.below(first, bound, level) {
                    return None;
                }
                let (element, after) = self.represented_at(first, deeper, catalog)?;
                (!self.below(after, bound, level)).then(|| (DataType::array(element, max), after))
            }
            _ => item.data_type(catalog).map(|t| (t, n.saturating_add(1))),
        }
    }

    /// [ISO/IEC 9075-2:2023 §20.11 GR5c, GR5d i; §20.12 GR4d, GR4e] Of the
    /// first COUNT items, each of LEVEL 0 and the type it represents with
    /// its subordinate items, if every one is valid and subordinate to
    /// one of LEVEL 0.
    pub(super) fn tops(&self, catalog: &str) -> Option<Vec<(i64, DataType)>> {
        let mut out = Vec::new();
        let mut n = 1_i64;
        while n <= self.count {
            if self.item(n).level() != 0 {
                return None;
            }
            let (t, next) = self.represented(n, self.count, catalog)?;
            out.push((n, t));
            n = next;
        }
        Some(out)
    }

    /// The numbers of the items immediately subordinate to item `n`, of
    /// type `t` as it represents it, among the first COUNT.
    fn immediate(&self, n: i64, t: &DataType, catalog: &str) -> Vec<(i64, DataType)> {
        let DataType::Row(r) = t else { return Vec::new() };
        let mut out = Vec::with_capacity(r.fields.len());
        let mut next = n.saturating_add(1);
        for f in &r.fields {
            out.push((next, f.data_type.clone()));
            next = self.represented(next, self.count, catalog).map_or(next, |(_, after)| after);
        }
        out
    }

    /// [ISO/IEC 9075-2:2023 §20.11 GR6b i 3] The associated value of item
    /// `n`, of type `t` as it represents it: null if INDICATOR is negative;
    /// of TYPE ROW, the row of its immediately subordinate items' values;
    /// else DATA. Types nest at most `MAX_TYPE_NESTING` deep, so this does
    /// too (RULES.md R-CF-6).
    ///
    /// # Errors
    ///
    /// SQLSTATE 07001 if a DATA it needs is undefined [GR5d ii].
    pub(super) fn associated(&self, n: i64, t: &DataType, catalog: &str) -> Result<Value> {
        let item = self.item(n);
        if item.number(DescriptorItem::Indicator) < 0 {
            return Ok(Value::Null);
        }
        if item.code() == code::ROW {
            let fields = self.immediate(n, t, catalog);
            let values = fields.iter().map(|(k, f)| self.associated(*k, f, catalog)).collect::<Result<Vec<_>>>()?;
            return Ok(Value::Row(Arc::from(values)));
        }
        let data = item.get(DescriptorItem::Data);
        if data.is_null() {
            let msg = format!("[ISO/IEC 9075-2:2023 §20.11 GR5d ii] the DATA of item {n} is undefined");
            return Err(dynamic_error(SqlState::USING_CLAUSE_MISMATCH, msg));
        }
        Ok(data)
    }

    /// [ISO/IEC 9075-2:2023 §20.12 GR5e i] Item `n`, of type `t` as it
    /// represents it, and its subordinate items set to reflect `tv`, cast
    /// from `sv` of type `sdt`: of TYPE ROW, null, INDICATOR -1 in it and
    /// in each subordinate item not subordinate to one of TYPE ARRAY, else
    /// each field in its item; else INDICATOR -1 of the null value, else 0
    /// and DATA `tv`, RETURNED_LENGTH and RETURNED_OCTET_LENGTH of a
    /// varying string's, and RETURNED_CARDINALITY of an array's. Types
    /// nest at most `MAX_TYPE_NESTING` deep, so this does too (RULES.md
    /// R-CF-6).
    pub(super) fn reflect(&mut self, n: i64, (t, catalog): (&DataType, &str), tv: &Value, (sv, sdt): (&Value, &DataType)) {
        let mut item = self.item(n);
        let fields = self.immediate(n, t, catalog);
        if item.code() != code::ROW {
            super::using::reflected(&mut item, (tv, t), (sv, sdt));
            self.items.insert(n, item);
            return;
        }
        // [DESIGN] Its own INDICATOR says whether the row is null, so a row
        // put after a null one is not taken for null.
        item.set(DescriptorItem::Indicator, exact(if tv.is_null() { -1 } else { 0 }));
        self.items.insert(n, item);
        let part = |v: &Value, i: usize| if let Value::Row(f) = v { f.get(i).cloned().unwrap_or(Value::Null) } else { Value::Null };
        let types: Vec<DataType> =
            if let DataType::Row(r) = sdt { r.fields.iter().map(|f| f.data_type.clone()).collect() } else { Vec::new() };
        for (i, (k, f)) in fields.iter().enumerate() {
            let its = types.get(i).cloned().unwrap_or_else(|| f.clone());
            self.reflect(*k, (f, catalog), &part(tv, i), (&part(sv, i), &its));
        }
    }

    /// Whether item `n` is subordinate to an item of TYPE ARRAY [§20.1 SR3].
    pub(super) fn under_array(&self, n: i64) -> bool {
        let mut level = self.item(n).level();
        for k in (1..n).rev() {
            let item = self.item(k);
            if item.level() < level {
                if item.code() == code::ARRAY {
                    return true;
                }
                level = item.level();
            }
            if level <= 0 {
                break;
            }
        }
        false
    }

    /// [ISO/IEC 9075-2:2023 §20.5 GR4b] Whether LEVEL `v` may be set in
    /// item `n`: 0 of the first; of another, one more than LEVEL `k` of the
    /// item before it if that one is of TYPE ROW or ARRAY, or else no more
    /// than `k`, each row it closes given all its fields \[A\] and the one
    /// the item is then a field of given fewer than DEGREE \[B\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 0700E if it may not.
    pub(super) fn level_may_be(&self, n: i64, v: i64) -> Result<()> {
        if n <= 1 {
            return if v == 0 {
                Ok(())
            } else {
                Err(invalid_level("GR4b i", &format!("LEVEL {v} is set in the first item, whose LEVEL is 0")))
            };
        }
        let previous = self.item(n.saturating_sub(1));
        let k = previous.level();
        if v == k.saturating_add(1) {
            return if [code::ROW, code::ARRAY].contains(&previous.code()) {
                Ok(())
            } else {
                Err(invalid_level("GR4b ii 1", &format!("LEVEL {v} follows an item of LEVEL {k} of TYPE neither ROW nor ARRAY")))
            };
        }
        if v > k.saturating_add(1) || v < 0 {
            return Err(invalid_level("GR4b ii 2", &format!("LEVEL {v} follows an item of LEVEL {k}")));
        }
        self.rows_closed(n, v)
    }

    /// [ISO/IEC 9075-2:2023 §20.5 GR4b ii 3] Of item `n` set to LEVEL `v`,
    /// no more than that of the item before it: each item of TYPE ROW that
    /// item is subordinate to, of LEVEL `v` or more, has its DEGREE of
    /// immediately subordinate items before `n` \[A\]; and, of `v` more
    /// than 0, the one of LEVEL `v` - 1, which `n` is then immediately
    /// subordinate to, is of TYPE ROW and has fewer \[B\]. A names the
    /// rows of LEVEL more than `v`, yet `n` follows the one of LEVEL `v`
    /// as well and so closes it too. B names the one of the LEVEL of the
    /// item before `n`, which none it is subordinate to has.
    fn rows_closed(&self, n: i64, v: i64) -> Result<()> {
        let mut level = self.item(n.saturating_sub(1)).level();
        let mut parent_found = v == 0;
        for k in (1..n).rev() {
            let item = self.item(k);
            if item.level() >= level {
                continue;
            }
            level = item.level();
            let fields = (k.saturating_add(1)..n).filter(|&j| self.item(j).level() == level.saturating_add(1)).count();
            let degree = usize::try_from(item.number(DescriptorItem::Degree)).unwrap_or(0);
            let row = item.code() == code::ROW;
            if row && level >= v && fields != degree {
                let msg = format!("LEVEL {v} closes the row of item {k} after {fields} of its DEGREE {degree} fields");
                return Err(invalid_level("GR4b ii 3 A", &msg));
            }
            if level == v.saturating_sub(1) && v > 0 {
                if !row || fields >= degree {
                    return Err(invalid_level("GR4b ii 3 B", &format!("item {n} of LEVEL {v} is no field of item {k}")));
                }
                parent_found = true;
            }
            if level <= 0 {
                break;
            }
        }
        if parent_found {
            Ok(())
        } else {
            Err(invalid_level("GR4b ii 3 B", &format!("no item of LEVEL {} comes before item {n}", v.saturating_sub(1))))
        }
    }
}
