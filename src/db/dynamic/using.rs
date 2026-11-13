//! [ISO/IEC 9075:1992 §17.9 GR5-GR7] `<using clause>`s: the values a
//! prepared statement's `<dynamic parameter specification>`s take, of
//! arguments or of an SQL descriptor area; and where the values of a row
//! go, to targets or to an SQL descriptor area's items: [ISO/IEC
//! 9075-2:2023 §20.11, §20.12] each value of an item of LEVEL 0 and those
//! subordinate to it (`nested.rs`).

use super::descriptor::{Descriptor, Item, exact, number};
use super::{Name, dynamic_error};
use crate::ast::{DescriptorItem, ParameterSpecification, UsingClause};
use crate::binder::{OutColumn, assignable, cast_is_valid};
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
    /// to the type of its item of LEVEL 0 [GR7b], of these numbers, in
    /// the catalog of this name.
    Descriptor(Name, Descriptor, Vec<(i64, DataType)>, String),
}

/// [ISO/IEC 9075:1992 §17.9 GR5e, GR7c] A value of `from` is not cast to
/// `to` (SQLSTATE 07006).
fn restricted(rule: &str, from: &DataType, to: &DataType) -> SqlError {
    dynamic_error(SqlState::RESTRICTED_DATA_TYPE, format!("[ISO/IEC 9075:1992 {rule}, §6.10 SR4] CAST of {from} to {to} is not valid"))
}

/// [ISO/IEC 9075:1992 §17.9 GR5e, GR7c] Whether a value of `from` goes
/// to `to`, as CAST takes it; [ISO/IEC 9075-2:2023 §20.11 GR6c iii, §20.12
/// GR5d iii] to a distinct type by its to-sql function, FNUDT, from one by
/// its from-sql function, FNSDT (ID048), whose source type is then
/// compatible with the other, each assignable to the other.
fn transformed(from: &DataType, to: &DataType) -> bool {
    let compatible = |a: &DataType, b: &DataType| assignable(a, b) && assignable(b, a);
    if let DataType::Distinct(d) = to {
        return compatible(&d.source, from);
    }
    if let DataType::Distinct(d) = from {
        return compatible(&d.source, to);
    }
    cast_is_valid(from, to)
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
/// RETURNED_OCTET_LENGTH of `v`, a varying string's: of a binary string,
/// its octets both.
fn returned(item: &mut Item, v: &Value) {
    let (length, octets) = match v {
        Value::Char(s) => (s.chars().count(), s.len()),
        Value::Binary(b) => (usize::try_from(b.len()).unwrap_or(usize::MAX), b.bytes().len()),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Extension(_) => {
            return;
        }
    };
    item.set(DescriptorItem::ReturnedLength, exact(i64::try_from(length).unwrap_or(i64::MAX)));
    item.set(DescriptorItem::ReturnedOctetLength, exact(i64::try_from(octets).unwrap_or(i64::MAX)));
}

/// Whether `t` is CHARACTER VARYING or BINARY VARYING.
fn varying(t: &DataType) -> bool {
    matches!(t, DataType::Varying { .. } | DataType::VarBinary { .. })
}

/// [ISO/IEC 9075:1992 §17.9 GR7e-g; ISO/IEC 9075-2:2023 §20.12 GR5e i 2]
/// `item`, of a type `t` not a row type, set to reflect `tv`, cast from
/// `sv` of type `sdt`: INDICATOR -1 of the null value, else 0 and DATA
/// `tv`, RETURNED_LENGTH and RETURNED_OCTET_LENGTH of a varying string's,
/// and RETURNED_CARDINALITY of an array's.
pub(super) fn reflected(item: &mut Item, (tv, t): (&Value, &DataType), (sv, sdt): (&Value, &DataType)) {
    if tv.is_null() {
        item.set(DescriptorItem::Indicator, exact(-1));
        return;
    }
    item.set(DescriptorItem::Indicator, exact(0));
    item.set(DescriptorItem::Data, tv.clone());
    if varying(t) {
        returned(item, tv);
    } else if varying(sdt) {
        returned(item, sv);
    }
    if let Value::Array(a) = tv {
        item.set(DescriptorItem::ReturnedCardinality, exact(number(a.len())));
    }
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
            .map(|((value, from), to)| if transformed(&from, to) { Ok(value) } else { Err(restricted("§17.9 GR5e", &from, &to.clone())) })
            .collect()
    }

    /// [ISO/IEC 9075:1992 §17.9 GR5b, GR5d i; ISO/IEC 9075-2:2023 §20.11
    /// GR5, GR6b i] The values of the `d` items of LEVEL 0 of the area
    /// `name` names, each with those subordinate to it, and their types.
    fn descriptor_values(&self, name: &crate::ast::ExtendedName, d: usize, params: &Params) -> Result<Vec<(Value, DataType)>> {
        let (_, area) = self.descriptor(name, params, "§17.9 GR2")?;
        check_count(area.count, area.occurrences, "§17.9 GR5b i")?;
        let catalog = self.catalog_now()?;
        let Some(tops) = area.tops(&catalog) else {
            return Err(parameters_mismatch("§17.9 GR5b iii, §17.1 SR3", "an item of the first COUNT describes no valid type"));
        };
        if tops.len() != d {
            let msg = format!("{} items of LEVEL 0 for {d} <dynamic parameter specification>s", tops.len());
            return Err(parameters_mismatch("§17.9 GR5b ii", &msg));
        }
        tops.into_iter().map(|(n, t)| Ok((area.associated(n, &t, &catalog)?, t))).collect()
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
        let catalog = self.catalog_now()?;
        let Some(tops) = area.tops(&catalog) else {
            return Err(targets_mismatch("§17.9 GR6b iii, §17.1 SR3", "an item of the first COUNT describes no valid type"));
        };
        if tops.len() != d {
            return Err(targets_mismatch("§17.9 GR6b ii", &format!("{} items of LEVEL 0 for {d} columns", tops.len())));
        }
        for ((_, t), column) in tops.iter().zip(columns) {
            if !transformed(&column.data_type, t) {
                return Err(restricted("§17.9 GR7c", &column.data_type.clone(), t));
            }
        }
        Ok(Targets::Descriptor(key, area, tops, catalog))
    }

    /// [ISO/IEC 9075:1992 §17.9 GR7; ISO/IEC 9075-2:2023 §20.12 GR5] The
    /// values `row` of `columns` put where `targets` says: assigned to the
    /// arguments; or each cast to the type of its item of LEVEL 0, which,
    /// with those subordinate to it, is set to reflect it (`nested.rs`).
    ///
    /// # Errors
    ///
    /// Those of the assignment, or of CAST [GR7d].
    pub(super) fn put_row(&self, targets: &Targets<'_>, columns: &[OutColumn], row: &[Value], params: &mut Params) -> Result<Completion> {
        let (key, mut area, tops, catalog) = match targets {
            Targets::Arguments(targets, (null_rule, assign_rule)) => return assign_targets(row, targets, params, null_rule, assign_rule),
            Targets::Descriptor(key, area, tops, catalog) => (key, area.clone(), tops, catalog),
        };
        for (((n, t), sv), column) in tops.iter().zip(row).zip(columns) {
            let tv = cast(sv.clone(), t)?;
            area.reflect(*n, (t, catalog), &tv, (sv, &column.data_type));
        }
        self.put_descriptor(key.clone(), area);
        Ok(Completion::Success)
    }
}
