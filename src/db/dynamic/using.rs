//! [ISO/IEC 9075:1992 §17.9 GR5-GR7] `<using clause>`s: the values a
//! prepared statement's `<dynamic parameter specification>`s take, of
//! arguments or of an SQL descriptor area; and where the values of a row
//! go, to targets or to an SQL descriptor area's items.

use super::descriptor::{Descriptor, Item, exact};
use super::{Name, dynamic_error};
use crate::ast::{DescriptorItem, ParameterSpecification, UsingClause};
use crate::binder::{OutColumn, cast_is_valid};
use crate::db::assignment::{assign_targets, check_targets};
use crate::db::{Completion, Session};
use crate::error::{Result, SqlError, SqlState};
use crate::params::Params;
use crate::value::{DataType, Value, cast};

/// Where the values of a row go [§17.9 GR6, GR7].
pub(super) enum Targets<'a> {
    /// To these targets, by retrieval assignment, as `rules` cite: the null
    /// value's and the assignment's.
    Arguments(&'a [ParameterSpecification], (&'static str, &'static str)),
    /// To the SQL descriptor area of this name, as it is, each value cast
    /// to the type of its item [GR7b].
    Descriptor(Name, Descriptor, Vec<DataType>),
}

/// [ISO/IEC 9075:1992 §17.9 GR5e, GR7c] A value of `from` is not cast to
/// `to` (SQLSTATE 07006).
fn restricted(rule: &str, from: DataType, to: DataType) -> SqlError {
    dynamic_error(SqlState::RESTRICTED_DATA_TYPE, format!("[ISO/IEC 9075:1992 {rule}, §6.10 SR4] CAST of {from} to {to} is not valid"))
}

/// [ISO/IEC 9075:1992 §17.9 GR5a, GR5b ii-iv] The `<using clause>` does
/// not match the dynamic parameters (SQLSTATE 07001): `msg`.
fn parameters_mismatch(rule: &str, msg: &str) -> SqlError {
    dynamic_error(SqlState::USING_CLAUSE_MISMATCH, format!("[ISO/IEC 9075:1992 {rule}] {msg}"))
}

/// [ISO/IEC 9075:1992 §17.9 GR6a, GR6b ii, iii] The `<using clause>` does
/// not match the targets (SQLSTATE 07002): `msg`.
fn targets_mismatch(rule: &str, msg: &str) -> SqlError {
    dynamic_error(SqlState::USING_CLAUSE_TARGETS_MISMATCH, format!("[ISO/IEC 9075:1992 {rule}] {msg}"))
}

/// [ISO/IEC 9075:1992 §17.9 GR5b i, GR6b i] COUNT of an area of
/// `occurrences` is less than 0 or more than them (SQLSTATE 07008).
fn check_count(count: i64, occurrences: i64, rule: &str) -> Result<()> {
    if (0..=occurrences).contains(&count) {
        return Ok(());
    }
    Err(dynamic_error(SqlState::INVALID_DESCRIPTOR_COUNT, format!("[ISO/IEC 9075:1992 {rule}] COUNT {count} is not of 0 to {occurrences}")))
}

/// [ISO/IEC 9075:1992 §17.9 GR7g ii] RETURNED_LENGTH and
/// RETURNED_OCTET_LENGTH of `v`, a varying string's.
fn returned(item: &mut Item, v: &Value) {
    let (length, octets) = match v {
        Value::Char(s) => (s.chars().count(), s.len()),
        Value::Bits(b) => (usize::try_from(b.len()).unwrap_or(usize::MAX), b.bytes().len()),
        Value::Null | Value::Exact(_) | Value::Approx(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => return,
    };
    item.set(DescriptorItem::ReturnedLength, exact(i64::try_from(length).unwrap_or(i64::MAX)));
    item.set(DescriptorItem::ReturnedOctetLength, exact(i64::try_from(octets).unwrap_or(i64::MAX)));
}

/// Whether `t` is CHARACTER VARYING or BIT VARYING.
fn varying(t: DataType) -> bool {
    matches!(t, DataType::Varying { .. } | DataType::BitVarying { .. })
}

impl Session {
    /// [ISO/IEC 9075:1992 §17.9 GR5] The values of a prepared statement's
    /// dynamic parameters, of types `types`, that `using` gives: each
    /// argument's, or each item descriptor area's DATA, or the null value
    /// if its INDICATOR is negative. None if there are none; else the
    /// clause is required, as `required` cites.
    ///
    /// # Errors
    ///
    /// SQLSTATE 07004 if there are parameters and no `using`; 07001 if
    /// `using` does not give one valid value for each \[GR5a, GR5b ii-iv\];
    /// 07008 if COUNT is not of the area's occurrences \[GR5b i\]; 07006 if
    /// a value's type cannot be cast to its parameter's \[GR5e\]; 33000 if
    /// no area of the name is allocated \[GR2\].
    pub(super) fn using_values(
        &self,
        using: Option<&UsingClause>,
        types: &[DataType],
        params: &Params,
        required: &str,
    ) -> Result<Vec<Value>> {
        let Some(using) = using else {
            if types.is_empty() {
                return Ok(Vec::new());
            }
            let msg = format!("[ISO/IEC 9075:1992 {required}] the statement has dynamic parameters, and no <using clause>");
            return Err(dynamic_error(SqlState::USING_CLAUSE_REQUIRED_PARAMETERS, msg));
        };
        let given: Vec<(Value, DataType)> = match using {
            UsingClause::Arguments(arguments) => {
                if arguments.len() != types.len() {
                    let msg = format!("{} values for {} <dynamic parameter specification>s", arguments.len(), types.len());
                    return Err(parameters_mismatch("§17.9 GR5a", &msg));
                }
                arguments.iter().map(|a| params.input(a)).collect::<Result<_>>()?
            }
            UsingClause::Descriptor(name) => self.descriptor_values(name, types.len(), params)?,
        };
        given
            .into_iter()
            .zip(types)
            .map(|((value, from), to)| if cast_is_valid(&from, to) { Ok(value) } else { Err(restricted("§17.9 GR5e", from, *to)) })
            .collect()
    }

    /// [ISO/IEC 9075:1992 §17.9 GR5b, GR5d i] The values of the first `d`
    /// item descriptor areas of the area `name` names, and their types.
    fn descriptor_values(&self, name: &crate::ast::ExtendedName, d: usize, params: &Params) -> Result<Vec<(Value, DataType)>> {
        let (_, area) = self.descriptor(name, params, "§17.9 GR2")?;
        check_count(area.count, area.occurrences, "§17.9 GR5b i")?;
        if usize::try_from(area.count).ok() != Some(d) {
            return Err(parameters_mismatch(
                "§17.9 GR5b ii",
                &format!("COUNT is {} for {d} <dynamic parameter specification>s", area.count),
            ));
        }
        let catalog = self.catalog_now()?;
        (1..=area.count)
            .map(|n| {
                let item = area.item(n);
                let Some(t) = item.data_type(&catalog) else {
                    return Err(parameters_mismatch("§17.9 GR5b iii, §17.1 SR3", &format!("item {n} describes no valid type")));
                };
                if item.number(DescriptorItem::Indicator) < 0 {
                    return Ok((Value::Null, t));
                }
                // DATA set is of the item's type [§17.5 GR5], and undefined
                // once another field is set [GR4]: defined, it is valid.
                let data = item.get(DescriptorItem::Data);
                if data.is_null() {
                    return Err(parameters_mismatch("§17.9 GR5b iv", &format!("the DATA of item {n} is undefined")));
                }
                Ok((data, t))
            })
            .collect()
    }

    /// [ISO/IEC 9075:1992 §17.9 GR6] Where the values of a row of `columns`
    /// go, as `using` says: its arguments, as many as the columns, each
    /// assignable from its column, assigned as `rules` cite; or the items
    /// of an SQL descriptor area, COUNT the columns' number, each valid,
    /// whose type a column's value can be cast to.
    ///
    /// # Errors
    ///
    /// SQLSTATE 07002 if `using` does not match the columns \[GR6a, GR6b
    /// ii, iii\]; 07008 if COUNT is not of the area's occurrences \[GR6b i\];
    /// 07006 if a column cannot be cast to its item's type [GR7c]; 33000 if
    /// no area of the name is allocated \[GR2\]; SQLCODE -103 if a column
    /// cannot be assigned to its target [§17.15 SR7, §9.1 SR2].
    pub(super) fn using_targets<'a>(
        &self,
        using: &'a UsingClause,
        columns: &[OutColumn],
        params: &Params,
        rules: (&'static str, &'static str),
    ) -> Result<Targets<'a>> {
        let d = columns.len();
        let name = match using {
            UsingClause::Arguments(targets) => {
                if targets.len() != d {
                    return Err(targets_mismatch("§17.9 GR6a", &format!("{} arguments for {d} columns", targets.len())));
                }
                check_targets(columns, targets, params, "§8.6 SR2", "§8.6 SR3")?;
                return Ok(Targets::Arguments(targets, rules));
            }
            UsingClause::Descriptor(name) => name,
        };
        let (key, area) = self.descriptor(name, params, "§17.9 GR2")?;
        check_count(area.count, area.occurrences, "§17.9 GR6b i")?;
        if usize::try_from(area.count).ok() != Some(d) {
            return Err(targets_mismatch("§17.9 GR6b ii", &format!("COUNT is {} for {d} columns", area.count)));
        }
        let catalog = self.catalog_now()?;
        let mut types = Vec::with_capacity(d);
        for (n, column) in (1..).zip(columns) {
            let Some(t) = area.item(n).data_type(&catalog) else {
                return Err(targets_mismatch("§17.9 GR6b iii, §17.1 SR3", &format!("item {n} describes no valid type")));
            };
            if !cast_is_valid(&column.data_type, &t) {
                return Err(restricted("§17.9 GR7c", column.data_type, t));
            }
            types.push(t);
        }
        Ok(Targets::Descriptor(key, area, types))
    }

    /// [ISO/IEC 9075:1992 §17.9 GR7] The values `row` of `columns` put where
    /// `targets` says: assigned to the arguments; or each cast to its item's
    /// type, INDICATOR -1 of the null value, else 0 and DATA the value, and
    /// RETURNED_LENGTH and RETURNED_OCTET_LENGTH of a varying string's.
    ///
    /// # Errors
    ///
    /// Those of the assignment, or of CAST [GR7d].
    pub(super) fn put_row(&self, targets: &Targets<'_>, columns: &[OutColumn], row: &[Value], params: &mut Params) -> Result<Completion> {
        let (key, mut area, types) = match targets {
            Targets::Arguments(targets, (null_rule, assign_rule)) => return assign_targets(row, targets, params, null_rule, assign_rule),
            Targets::Descriptor(key, area, types) => (key, area.clone(), types),
        };
        for ((n, (sv, t)), column) in (1..).zip(row.iter().zip(types)).zip(columns) {
            let tv = cast(sv.clone(), t)?;
            let mut item = area.item(n);
            if tv.is_null() {
                item.set(DescriptorItem::Indicator, exact(-1));
            } else {
                item.set(DescriptorItem::Indicator, exact(0));
                item.set(DescriptorItem::Data, tv.clone());
                if varying(*t) {
                    returned(&mut item, &tv);
                } else if varying(column.data_type) {
                    returned(&mut item, sv);
                }
            }
            area.items.insert(n, item);
        }
        self.put_descriptor(key.clone(), area);
        Ok(Completion::Success)
    }
}
