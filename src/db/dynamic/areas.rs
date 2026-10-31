//! [ISO/IEC 9075:1992 §17.2-§17.5] The statements of SQL descriptor areas:
//! ALLOCATE, DEALLOCATE, GET and SET DESCRIPTOR; and [§17.9 GR2-GR4]
//! DESCRIBE's filling of one.

use super::descriptor::{Descriptor, Item, code, exact, is_text, matches};
use super::{Name, dynamic_error, value_of, whole};
use crate::ast::{DescriptorItem, ExtendedName, GetDescriptor, ParameterSpecification, SetDescriptor, ValueSpecification};
use crate::db::assignment::declared_type;
use crate::db::{Completion, Session};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::params::Params;
use crate::value::{DEFAULT_EXACT_PRECISION, DOUBLE_PRECISION, DataType, Value, assign};

/// [ISO/IEC 9075:1992 §17.2 SR2] The `<occurrences>` of an SQL descriptor
/// area allocated without WITH MAX: implementation-defined, greater than 0.
pub(crate) const DEFAULT_OCCURRENCES: i64 = 100;
/// [ISO/IEC 9075:1992 §17.2 GR3] The most `<occurrences>` an SQL
/// descriptor area may have: implementation-defined.
pub(crate) const MAX_OCCURRENCES: i64 = 1_000;
/// [ISO/IEC 9075:1992 §17.2 GR3] The most SQL descriptor areas a session
/// has allocated at one time: implementation-defined. One more is valid
/// SQL beyond a limit (RULES.md D18).
pub(crate) const MAX_DESCRIPTORS: usize = 64;

/// [ISO/IEC 9075:1992 §17.5 GR4] The order a `<set descriptor statement>`
/// assigns its fields in.
const SET_ORDER: [DescriptorItem; 11] = [
    DescriptorItem::Type,
    DescriptorItem::DatetimeIntervalCode,
    DescriptorItem::DatetimeIntervalPrecision,
    DescriptorItem::Precision,
    DescriptorItem::Scale,
    DescriptorItem::CharacterSetCatalog,
    DescriptorItem::CharacterSetSchema,
    DescriptorItem::CharacterSetName,
    DescriptorItem::Length,
    DescriptorItem::Indicator,
    DescriptorItem::Data,
];

/// The fractional second's precision TIMESTAMP takes when
/// DATETIME_INTERVAL_CODE is set [§17.5 GR5i ii], and an interval ending
/// in SECOND [GR5j i]; the leading field precision of an interval [GR5d,
/// GR5j].
const TIMESTAMP_PRECISION: i64 = 6;
const SECOND_PRECISION: i64 = 6;
const LEADING_PRECISION: i64 = 2;

/// The Table 20 codes of the `<interval qualifier>`s that end in SECOND
/// [§17.5 GR5j i].
const TO_SECOND: [i64; 4] = [6, 10, 12, 13];

/// The Table 19 codes of TIMESTAMP and TIMESTAMP WITH TIME ZONE [§17.5
/// GR5i ii].
const TIMESTAMPS: [i64; 2] = [3, 5];

/// [ISO/IEC 9075:1992 §17.3 GR1, §17.4 GR1, §17.5 GR1, §17.9 GR2] No SQL
/// descriptor area named `name` is allocated, as `rule` says.
fn not_allocated(name: &Name, rule: &str) -> SqlError {
    dynamic_error(SqlState::INVALID_DESCRIPTOR_NAME, format!("[ISO/IEC 9075:1992 {rule}] no SQL descriptor area '{name}' is allocated"))
}

/// [ISO/IEC 9075:1992 §17.4 GR3, §17.5 GR2] `n` is not the number of an
/// item descriptor area of one of `occurrences`.
fn bad_index(n: i64, occurrences: i64, rule: &str) -> SqlError {
    dynamic_error(
        SqlState::INVALID_DESCRIPTOR_INDEX,
        format!("[ISO/IEC 9075:1992 {rule}] item {n} is not of 1 to {occurrences}, the area's occurrences"),
    )
}

/// [ISO/IEC 9075:1992 §17.4 SR2, SR3; §17.5 SR2, SR3] A value or target of
/// a field shall be of its type in Table 17: `what` is not.
fn wrong_type(rule: &str, what: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 {rule}, Table 17] {what}"))
}

/// [ISO/IEC 9075:1992 §17.4 GR2, §17.5 GR5] A DATA value or target whose
/// type does not match the item descriptor area's (SQLSTATE 22005).
fn data_mismatch(rule: &str, t: DataType) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075:1992 {rule}, §17.1 SR2] {t} does not match the type the item describes"))
        .with_state(SqlState::ERROR_IN_ASSIGNMENT)
}

/// [ISO/IEC 9075:1992 §17.4 GR7] Each value got assigned to its target,
/// of a type its field's is assigned to [SR2, SR3, GR2].
fn got_into(got: Vec<(Value, ParameterSpecification)>, params: &mut Params) -> Result<Completion> {
    for (value, target) in got {
        let assigned = assign(value, &declared_type(params, &target.name)?, "ISO/IEC 9075:1992 §17.4 GR7").or_at(target.at)?;
        params.set(&target.name, assigned);
    }
    Ok(Completion::Success)
}

/// Whether `t` is of the type Table 17 gives a field that is not DATA: a
/// character string of at least 128 characters for a name's, else exact
/// numeric with scale 0; of a target, `target`, or of a value.
fn of_field_type(f: DescriptorItem, t: DataType, target: bool) -> bool {
    /// [ISO/IEC 9075:1992 §17.1 Table 17] The least length of a field of a
    /// name.
    const NAME_LENGTH: u32 = 128;
    if !is_text(f) {
        return matches!(t, DataType::Exact { scale: 0, .. });
    }
    match t {
        DataType::Character { length } | DataType::Varying { length } => !target || length >= NAME_LENGTH,
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => false,
    }
}

impl Session {
    /// The name of the SQL descriptor area `name` names, as `rule` checks it.
    pub(super) fn descriptor_name(&self, name: &ExtendedName, params: &Params, rule: &str) -> Result<Name> {
        self.extended(name, params, (SqlState::INVALID_DESCRIPTOR_NAME, rule))
    }

    /// The SQL descriptor area `name` names, allocated, as `rule` says.
    pub(super) fn descriptor(&self, name: &ExtendedName, params: &Params, rule: &str) -> Result<(Name, Descriptor)> {
        let key = self.descriptor_name(name, params, rule)?;
        let found = self.inner.borrow().dynamic.descriptors.get(&key).cloned();
        let area = found.ok_or_else(|| not_allocated(&key, rule))?;
        Ok((key, area))
    }

    /// The SQL descriptor area named `key` made `area`.
    pub(super) fn put_descriptor(&self, key: Name, area: Descriptor) {
        self.inner.borrow_mut().dynamic.descriptors.insert(key, area);
    }

    /// [ISO/IEC 9075:1992 §17.2] ALLOCATE DESCRIPTOR: an SQL descriptor
    /// area of `max` item descriptor areas, or `DEFAULT_OCCURRENCES` \[SR2\],
    /// each undefined \[GR2\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 33000 if the name is no `<identifier>` \[GR1\] or one
    /// allocated \[GR2\]; 07009 if `max` is less than 1 or more than
    /// `MAX_OCCURRENCES` \[GR3\]; SQLCODE -900 if the session has
    /// `MAX_DESCRIPTORS` already.
    pub(super) fn allocate_descriptor(&self, name: &ExtendedName, max: Option<&ValueSpecification>, params: &Params) -> Result<Completion> {
        let key = self.descriptor_name(name, params, "§17.2 GR1")?;
        let occurrences = max.map_or(Ok(DEFAULT_OCCURRENCES), |v| whole(self, v, params, "§17.2 SR1"))?;
        let held = self.inner.borrow().dynamic.descriptors.len();
        if self.inner.borrow().dynamic.descriptors.contains_key(&key) {
            return Err(dynamic_error(
                SqlState::INVALID_DESCRIPTOR_NAME,
                format!("[ISO/IEC 9075:1992 §17.2 GR2] SQL descriptor area '{key}' is allocated already"),
            ));
        }
        if !(1..=MAX_OCCURRENCES).contains(&occurrences) {
            let msg = format!("[ISO/IEC 9075:1992 §17.2 GR3] {occurrences} occurrences: an area has 1 to {MAX_OCCURRENCES}");
            return Err(dynamic_error(SqlState::INVALID_DESCRIPTOR_INDEX, msg));
        }
        if held >= MAX_DESCRIPTORS {
            let msg = format!(
                "[DESIGN] a session has at most {MAX_DESCRIPTORS} SQL descriptor areas at one time [ISO/IEC 9075:1992 §17.2 GR3] (RULES.md D18)"
            );
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        self.put_descriptor(key, Descriptor::new(occurrences));
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.3] DEALLOCATE DESCRIPTOR.
    ///
    /// # Errors
    ///
    /// SQLSTATE 33000 if no area of the name is allocated \[GR1\].
    pub(super) fn deallocate_descriptor(&self, name: &ExtendedName, params: &Params) -> Result<Completion> {
        let (key, _) = self.descriptor(name, params, "§17.3 GR1")?;
        self.inner.borrow_mut().dynamic.descriptors.remove(&key);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.4] GET DESCRIPTOR: COUNT, or fields of an
    /// item descriptor area, assigned to their targets \[GR7\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 33000 if no area of the name is allocated \[GR1\]; SQLCODE
    /// -103 if a target is not of its field's type \[SR2, SR3\]; 22005 if
    /// DATA's is not the item's [GR2]; 07009 if the item number is not of
    /// the area's occurrences \[GR3\]; 22002 if DATA is got of a negative
    /// INDICATOR, not got \[GR5\]; those of assignment.
    pub(super) fn get_descriptor(&self, name: &ExtendedName, info: &GetDescriptor, params: &mut Params) -> Result<Completion> {
        let (_, area) = self.descriptor(name, params, "§17.4 GR1")?;
        let (number, items) = match info {
            GetDescriptor::Count(target) => {
                if !of_field_type(DescriptorItem::Type, declared_type(params, &target.name)?, true) {
                    return Err(wrong_type("§17.4 SR2", "the target of COUNT is exact numeric with scale 0"));
                }
                return got_into(vec![(exact(area.count), target.clone())], params);
            }
            GetDescriptor::Value { number, items } => (whole(self, number, params, "§17.4 SR1")?, items),
        };
        if !(1..=area.occurrences).contains(&number) {
            return Err(bad_index(number, area.occurrences, "§17.4 GR3"));
        }
        if number > area.count {
            // [§17.4 GR4] beyond COUNT: no data.
            return Ok(Completion::NoData);
        }
        let item = area.item(number);
        let catalog = self.catalog_now()?;
        let mut got: Vec<(Value, ParameterSpecification)> = Vec::new();
        let indicator = items.iter().any(|(_, f)| *f == DescriptorItem::Indicator);
        for (target, f) in items {
            let t = declared_type(params, &target.name)?;
            if *f != DescriptorItem::Data {
                if !of_field_type(*f, t, true) {
                    return Err(wrong_type("§17.4 SR3", &format!("the target of {} is not of its type", f.name())));
                }
                got.push((item.get(*f), target.clone()));
                continue;
            }
            if !item.data_type(&catalog).is_some_and(|described| matches(described, t, true)) {
                return Err(data_mismatch("§17.4 GR2", t));
            }
            let negative = item.number(DescriptorItem::Indicator) < 0;
            if negative && !indicator {
                let msg = "[ISO/IEC 9075:1992 §17.4 GR5] INDICATOR is negative, and DATA is got without it";
                return Err(SqlError::new(Code::Data, msg).with_state(SqlState::NULL_VALUE_NO_INDICATOR));
            }
            // [§17.4 GR7d] DATA is undefined if INDICATOR is negative: its
            // target is left as it is.
            if !negative {
                got.push((item.get(*f), target.clone()));
            }
        }
        got_into(got, params)
    }

    /// [ISO/IEC 9075:1992 §17.5] SET DESCRIPTOR: COUNT, or fields of an item
    /// descriptor area, in the order GR4 gives, each set as GR5 says.
    ///
    /// # Errors
    ///
    /// SQLSTATE 33000 if no area of the name is allocated \[GR1\]; SQLCODE
    /// -103 if a value is not of its field's type \[SR2, SR3\]; 07009 if the
    /// item number is not of the area's occurrences \[GR2\]; 22005 if DATA's
    /// type does not match the item's \[GR5\].
    pub(super) fn set_descriptor(&self, name: &ExtendedName, info: &SetDescriptor, params: &Params) -> Result<Completion> {
        let (key, mut area) = self.descriptor(name, params, "§17.5 GR1")?;
        let (number, items) = match info {
            SetDescriptor::Count(v) => {
                area.count = whole(self, v, params, "§17.5 SR2")?;
                self.put_descriptor(key, area);
                return Ok(Completion::Success);
            }
            SetDescriptor::Value { number, items } => (whole(self, number, params, "§17.5 SR1")?, items),
        };
        if !(1..=area.occurrences).contains(&number) {
            return Err(bad_index(number, area.occurrences, "§17.5 GR2"));
        }
        let catalog = self.catalog_now()?;
        let mut item = area.item(number);
        for f in SET_ORDER {
            let Some((_, v)) = items.iter().find(|(set, _)| *set == f) else { continue };
            let (value, t) = value_of(self, v, params)?;
            if f == DescriptorItem::Data {
                if !item.data_type(&catalog).is_some_and(|described| matches(described, t, false)) {
                    return Err(data_mismatch("§17.5 GR5", t));
                }
                item.set(f, value);
                continue;
            }
            if !of_field_type(f, t, false) {
                return Err(wrong_type("§17.5 SR3", &format!("the value of {} is not of its type", f.name())));
            }
            set_field(&mut item, f, value, &catalog);
        }
        area.items.insert(number, item);
        self.put_descriptor(key, area);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.9 GR2-GR4] The SQL descriptor area `name`
    /// names described as `items`: COUNT their number, and they its first
    /// item descriptor areas, unless it has fewer occurrences; then the
    /// warning insufficient item descriptor areas (01005).
    ///
    /// # Errors
    ///
    /// SQLSTATE 33000 if no area of the name is allocated \[GR2\].
    pub(super) fn describe_into(&self, name: &ExtendedName, params: &Params, items: Vec<Item>) -> Result<Completion> {
        let (key, mut area) = self.descriptor(name, params, "§17.9 GR2")?;
        let insufficient = area.describe(items);
        self.put_descriptor(key, area);
        Ok(if insufficient { Completion::Warning(SqlState::INSUFFICIENT_ITEM_DESCRIPTOR_AREAS) } else { Completion::Success })
    }

    /// The name of the one catalog, as the definitions now give it.
    pub(super) fn catalog_now(&self) -> Result<String> {
        Ok(self.db.snapshot()?.catalog_name())
    }
}

/// [ISO/IEC 9075:1992 §17.5 GR4, GR5] Field `f`, not DATA, of `item` set
/// to `value`; then the fields GR5 sets with it, and DATA undefined
/// \[GR4\].
fn set_field(item: &mut Item, f: DescriptorItem, value: Value, catalog: &str) {
    item.set(f, value);
    item.forget(DescriptorItem::Data);
    if f == DescriptorItem::Type {
        set_type(item, catalog);
    } else if f == DescriptorItem::DatetimeIntervalCode {
        set_interval_code(item);
    }
}

/// [ISO/IEC 9075:1992 §17.5 GR5a-h] TYPE set: the fields its type takes
/// set as GR5 says, every other undefined.
fn set_type(item: &mut Item, catalog: &str) {
    item.keep_only(&[DescriptorItem::Type]);
    match item.number(DescriptorItem::Type) {
        code::CHARACTER | code::VARYING => {
            item.default_character_set(catalog);
            item.set_number(DescriptorItem::Length, 1);
        }
        code::BIT | code::BIT_VARYING => item.set_number(DescriptorItem::Length, 1),
        code::DATETIME => item.set_number(DescriptorItem::Precision, 0),
        code::INTERVAL => item.set_number(DescriptorItem::DatetimeIntervalPrecision, LEADING_PRECISION),
        code::NUMERIC | code::DECIMAL => {
            item.set_number(DescriptorItem::Scale, 0);
            item.set_number(DescriptorItem::Precision, i64::from(DEFAULT_EXACT_PRECISION));
        }
        // [§17.5 GR5g] FLOAT's precision when it is written without one.
        code::FLOAT => item.set_number(DescriptorItem::Precision, i64::from(DOUBLE_PRECISION)),
        _ => {}
    }
}

/// [ISO/IEC 9075:1992 §17.5 GR5i, GR5j] DATETIME_INTERVAL_CODE set: of a
/// datetime, PRECISION 0, or 6 of a TIMESTAMP; of an interval,
/// DATETIME_INTERVAL_PRECISION 2, and PRECISION 6 if it ends in SECOND,
/// else 0. DATA undefined.
fn set_interval_code(item: &mut Item) {
    let dic = item.number(DescriptorItem::DatetimeIntervalCode);
    match item.number(DescriptorItem::Type) {
        code::DATETIME => item.set_number(DescriptorItem::Precision, if TIMESTAMPS.contains(&dic) { TIMESTAMP_PRECISION } else { 0 }),
        code::INTERVAL => {
            item.set_number(DescriptorItem::DatetimeIntervalPrecision, LEADING_PRECISION);
            item.set_number(DescriptorItem::Precision, if TO_SECOND.contains(&dic) { SECOND_PRECISION } else { 0 });
        }
        _ => {}
    }
}
