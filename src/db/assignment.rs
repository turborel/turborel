//! The rows a statement writes and the values it returns: NOT NULL and UNIQUE
//! checks [X3.135-1986 §4.5, §6.3, §6.4], WITH CHECK OPTION [X3.135-1986
//! §6.5 GR3], the candidate row of an UPDATE
//! [X3.135-1986 §8.11, §8.12], and assignment to FETCH and SELECT targets
//! [X3.135-1986 §8.6, §8.10]. Split from db.rs (RULES.md R-SIZE-2).

use super::{Completion, State, api, rule, table_desc};
use crate::ast::ParameterSpecification;
use crate::binder::{self, OutColumn, ViewCheck};
use crate::catalog::{CheckLevels, ConstraintKind, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError, SqlState, Subject, fail, fail_with};
use crate::exec::{Subqueries, eval, eval_cond};
use crate::index::Key;
use crate::logical::{Cond, Truth};
use crate::params::Params;
use crate::storage::{Newest, Row, RowId, project};
use crate::txn::TxId;
use crate::value::cast::cast;
use crate::value::{DataType, Decimal, Value, assign};

/// [ISO/IEC 9075:1992 §11.19 GR11] Rows inserted or updated by an update
/// operation, which spans the views WITH CHECK OPTION `checks`, are rows of
/// each [X3.135-1986 §6.5 GR3a; `rule`, one of §8.7 GR2, §8.11 GR5, §8.12
/// GR4], their subqueries run by `subqueries`. \[GR12\] They are checked at
/// the end of the statement, once each row is changed: the subqueries may
/// read the table changed [§7.9 SR12d].
pub(super) fn check_option<'r>(
    checks: &[ViewCheck],
    rows: impl IntoIterator<Item = &'r Row>,
    (rule, subqueries): (&str, &dyn Subqueries),
) -> Result<()> {
    if checks.is_empty() {
        return Ok(());
    }
    for row in rows {
        for check in checks {
            check_row(check, row, rule, subqueries)?;
        }
    }
    Ok(())
}

/// `check_option` of one row, against one view: a view CASCADED refuses a
/// row that does not appear in it \[GR11a\]; one LOCAL a row that appears
/// in the table its query reads but not in it \[GR11b\].
fn check_row(check: &ViewCheck, row: &Row, rule: &str, subqueries: &dyn Subqueries) -> Result<()> {
    let holds = |cond: &Option<Cond>| cond.as_ref().map_or(Ok(true), |c| eval_cond(c, row, subqueries).map(|t| t == Truth::True));
    let refused = match check.levels {
        CheckLevels::Cascaded => !(holds(&check.own)? && holds(&check.below)?),
        CheckLevels::Local => !holds(&check.own)? && holds(&check.below)?,
    };
    if !refused {
        return Ok(());
    }
    let (view, levels) = (&check.view, if check.levels == CheckLevels::Local { "LOCAL" } else { "CASCADED" });
    let msg =
        format!("[ISO/IEC 9075:1992 §11.19 GR11; X3.135-1986 {rule}] the row is not in view {view}, defined WITH {levels} CHECK OPTION");
    // [§18.1 GR3i] about the view.
    Err(SqlError::new(Code::Constraint, msg).with_state(SqlState::WITH_CHECK_OPTION_VIOLATION).about(Subject::table(view.parts())))
}

/// Whether a row whose newest state is `newest` holds `key`, as `key_of`
/// finds it; refused with SQLCODE -911 while another transaction's change
/// to it leaves it in doubt.
fn holds<F: Fn(&Row) -> Result<Key>>(newest: &Newest, key: &Key, key_of: &F) -> Result<bool> {
    let has = |row: &Option<Row>| row.as_ref().map(key_of).transpose().map(|k| k.as_ref() == Some(key));
    let doubt = match &newest.pending {
        Some(pending) => has(&newest.row)? || has(pending)?,
        None => false,
    };
    if doubt {
        return fail(
            Code::Conflict,
            "[DESIGN] another transaction is changing a row with the same key; run the statement again".to_owned(),
        );
    }
    has(&newest.row)
}

/// The message of a duplicate of `columns` of the table `desc`: its
/// UNIQUE or PRIMARY KEY constraint named, as GET DIAGNOSTICS names it.
fn duplicated(desc: &TableDesc, columns: &[usize]) -> String {
    let names: Vec<&str> = columns.iter().filter_map(|&c| desc.columns.get(c)).map(|c| c.name.as_str()).collect();
    let constraint = desc.constraints.iter().find_map(|c| match &c.kind {
        ConstraintKind::Unique { columns: on, primary } => {
            (on == columns).then_some((if *primary { "PRIMARY KEY" } else { "UNIQUE" }, &c.name))
        }
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => None,
    });
    let (kind, constraint_name) = constraint.map_or(("UNIQUE", String::new()), |(k, n)| (k, format!(" {n}")));
    format!(
        "[X3.135-1986 §6.4 GR2] {kind}{constraint_name} ({}) of {} would be violated: a row of these values is there",
        names.join(", "),
        desc.name
    )
}

/// [X3.135-1986 §6.4 GR2] UNIQUE on `columns` of `table`, "effectively
/// checked after the execution of each `<SQL statement>`" [X3.135-1986
/// §4.5], or deferred [ISO/IEC 9075:1992 §10.6]: rows may pass through
/// duplicate keys meanwhile (as in SET ID = ID + 1), but not end with
/// them. `ids` are the rows written, through the index of the constraint.
///
/// \[DESIGN\] The rows counted are those whose newest version has the key,
/// whichever transaction committed it, as `tx` must not commit a duplicate
/// of a row it cannot see (RULES.md D29). A row that another open
/// transaction is changing, with the key before or after, is in doubt: the
/// statement is refused with SQLCODE -911, to be run again once that
/// transaction ends.
pub(super) fn check_key(st: &State, tx: TxId, table: &QualifiedName, columns: &[usize], ids: &[RowId]) -> Result<()> {
    let desc = table_desc(st, table)?;
    let storage = st.storage_of(table)?;
    for ix in st.catalog.indexes(table).into_iter().filter(|ix| ix.unique && ix.columns == columns) {
        let index = st.index_of(&ix.name)?;
        let key_of = |row: &Row| {
            project(row, &ix.columns).map(Key).ok_or_else(|| api(format!("[DESIGN] a row of {table} is shorter than its columns")))
        };
        for &id in ids {
            let Some(row) = storage.get(tx, id)? else { continue };
            let key = key_of(&row)?;
            // A key with a null is never a duplicate [X3.135-1986 §6.4 GR2].
            if key.0.iter().any(Value::is_null) {
                continue;
            }
            let holders = index
                .rows(&key)?
                .into_iter()
                .map(|other| holds(&storage.newest(tx, other)?, &key, &key_of))
                .try_fold(0_usize, |n, held| held.map(|held| n.saturating_add(usize::from(held))));
            match holders {
                Ok(n) if n > 1 => return fail(Code::Constraint, duplicated(&desc, &ix.columns)),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

/// The columns the `<set clause>`s of `w` give values to.
pub(super) fn set_columns(w: &binder::BoundWrite) -> Vec<usize> {
    w.set.iter().map(|(c, _)| *c).collect()
}

/// [X3.135-1986 §8.11 GR3-GR4, §8.12 GR2-GR3] the candidate row: a copy of the
/// object row with each SET value, every value computed from the object row
/// "before any value of the object row is updated", its subqueries run by
/// `subqueries`.
pub(super) fn candidate(w: &binder::BoundWrite, old: &Row, (assign_rule, subqueries): (&str, &dyn Subqueries)) -> Result<Row> {
    let mut new = old.clone();
    for (i, value) in &w.set {
        let v = match value {
            None => Value::Null,
            Some(e) => eval(e, old, subqueries)?,
        };
        let (Some(slot), Some(column)) = (new.get_mut(*i), w.desc.columns.get(*i)) else {
            return Err(api(format!("[DESIGN] {} has no column {i}", w.desc.name)));
        };
        *slot = assign(v, &column.data_type, assign_rule)?;
    }
    Ok(new)
}

/// [X3.135-1986 §8.6 SR2-SR3, §8.10 SR3-SR4] as many targets as columns, and
/// each column's type assignable to its target.
pub(super) fn check_targets(
    columns: &[OutColumn],
    targets: &[ParameterSpecification],
    params: &Params,
    count_rule: &str,
    type_rule: &str,
) -> Result<()> {
    if columns.len() != targets.len() {
        // At the first target too many, or the first of too few.
        let at = targets.get(columns.len()).or(targets.first()).map_or(0, |t| t.at);
        let msg = format!("[X3.135-1986 {count_rule}] {} targets for {} columns", targets.len(), columns.len());
        return Err(rule(msg)).or_at(at);
    }
    for (c, t) in columns.iter().zip(targets) {
        check_target(c, t, params, type_rule).or_at(t.at)?;
    }
    Ok(())
}

/// Whether column `c` can be assigned to target `t`, as `type_rule` says.
fn check_target(c: &OutColumn, t: &ParameterSpecification, params: &Params, type_rule: &str) -> Result<()> {
    let target = declared_type(params, &t.name)?;
    // [ISO/IEC 9075:1992 §9.1 SR2] a character string to a character
    // string, a number to a number: SQL-86's rule that only an exact one go
    // to an exact target is Entry SQL's [§13.3, §13.5, Leveling Rules].
    if target.is_character() != c.data_type.is_character() {
        return Err(rule(format!(
            "[X3.135-1986 {type_rule}; ISO/IEC 9075:1992 §9.1 SR2] a {} column cannot be assigned to {} ({target})",
            c.data_type, t.name
        )));
    }
    if let Some(ind) = &t.indicator {
        params.check_indicator(ind)?;
    }
    Ok(())
}

/// [X3.135-1986 §8.6 GR8-GR10, §8.10 GR7-GR9] assign a row's values to
/// targets; the completion is the first warning an assignment raises, if
/// one does [ISO/IEC 9075:1992 §9.1 GR3b, GR3e, GR3g, GR3h, GR3j].
pub(super) fn assign_targets(
    values: &[Value],
    targets: &[ParameterSpecification],
    params: &mut Params,
    null_rule: &str,
    assign_rule: &str,
) -> Result<Completion> {
    let mut warning = None;
    for (v, t) in values.iter().zip(targets) {
        let raised = assign_target(v, t, params, (null_rule, assign_rule)).or_at(t.at)?;
        warning = warning.or(raised);
    }
    Ok(warning.map_or(Completion::Success, Completion::Warning))
}

/// [X3.135-1986 §8.6 GR8-GR10, §8.10 GR7-GR9] assigns `fetched` to the target `spec`,
/// citing `null_rule` and `assign_rule`; the warning it raises, if any.
fn assign_target(
    fetched: &Value,
    spec: &ParameterSpecification,
    params: &mut Params,
    (null_rule, assign_rule): (&str, &str),
) -> Result<Option<SqlState>> {
    let int = |n: i64| Value::Exact(Decimal::new(i128::from(n), 0));
    let target = declared_type(params, &spec.name)?;
    if fetched.is_null() {
        // "If v is the null value, then an indicator shall be specified
        // for V, and that indicator is set to -1."
        let Some(ind) = &spec.indicator else {
            return fail_with(
                Code::Data,
                SqlState::NULL_VALUE_NO_INDICATOR,
                format!("[X3.135-1986 {null_rule}] the value is null, and target {} has no indicator parameter", spec.name),
            );
        };
        params.set(ind, int(-1));
        return Ok(None);
    }
    let (value, indicator, warning) = retrieved(fetched, &target, assign_rule)?;
    params.set(&spec.name, value);
    if let Some(ind) = &spec.indicator {
        params.set(ind, int(indicator));
    }
    Ok(warning)
}

/// [ISO/IEC 9075:1992 §9.1 GR2, GR3] `fetched`, not null, as a target of
/// type `target` takes it: the value, the target's indicator, and the
/// warning raised, if any; `assign_rule` cited of a number.
fn retrieved(fetched: &Value, target: &DataType, assign_rule: &str) -> Result<(Value, i64, Option<SqlState>)> {
    let cut = Some(SqlState::STRING_TRUNCATED);
    match (target, fetched) {
        // [X3.135-1986 §8.6 GR10a-c; §9.1 GR3a-c] the first L characters,
        // or padded with spaces to length L.
        (DataType::Character { length }, Value::Char(s)) => {
            let (m, l) = (s.chars().count(), usize::try_from(*length).unwrap_or(usize::MAX));
            if m > l {
                return Ok((Value::Char(s.chars().take(l).collect()), indicator_of(m)?, cut));
            }
            Ok((Value::Char(format!("{s}{}", " ".repeat(l.saturating_sub(m)))), 0, None))
        }
        // [§9.1 GR3d, GR3e] as it is, or its first L characters.
        (DataType::Varying { length }, Value::Char(s)) => {
            let (m, l) = (s.chars().count(), usize::try_from(*length).unwrap_or(usize::MAX));
            if m > l {
                return Ok((Value::Char(s.chars().take(l).collect()), indicator_of(m)?, cut));
            }
            Ok((Value::Char(s.clone()), 0, None))
        }
        // [§9.1 GR3f-GR3j] as it is, its first L bits, or a fixed length's
        // padded with 0 bits, as CAST fits them [§6.10 GR7].
        (DataType::Bit { length } | DataType::BitVarying { length }, Value::Bits(b)) => {
            let value = cast(fetched.clone(), target)?;
            if b.len() > *length {
                return Ok((value, indicator_of(usize::try_from(b.len()).unwrap_or(usize::MAX))?, cut));
            }
            let padded = b.len() < *length && matches!(target, DataType::Bit { .. });
            Ok((value, 0, padded.then_some(SqlState::ZERO_BIT_PADDING)))
        }
        // [X3.135-1986 §8.6 GR10d, GR10e; §9.1 GR3k] exact, approximate.
        (target_type, value) => {
            let case = if let DataType::Approximate { .. } = target_type { "e" } else { "d" };
            Ok((assign(value.clone(), target_type, &format!("X3.135-1986 {assign_rule}{case}; ISO/IEC 9075:1992 §9.1 GR3k"))?, 0, None))
        }
    }
}

/// [ISO/IEC 9075:1992 §9.1 GR2a] the indicator of a string of length `m`
/// cut to fit.
fn indicator_of(m: usize) -> Result<i64> {
    i64::try_from(m).map_err(|_| {
        SqlError::new(Code::Data, format!("[ISO/IEC 9075:1992 §9.1 GR2a] a value of length {m} is too long for its indicator"))
            .with_state(SqlState::INDICATOR_OVERFLOW)
    })
}

/// The declared type of the parameter `name`. The parser has checked that it
/// is declared [X3.135-1986 §7.3 SR3]; this reports it if not.
pub(super) fn declared_type(params: &Params, name: &str) -> Result<DataType> {
    params
        .get(name)
        .map(|p| p.data_type)
        .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §7.3 SR3] parameter {name} is not declared")))
}

// Unit tests: the checks behind assignment (RULES.md R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ColumnDesc, TableDesc};
    use crate::engine::rows::ROW_STORE;
    use crate::exec::NoSubqueries;
    use crate::logical::LogicalPlan;
    use crate::value::Spelled;

    /// Verifies: [X3.135-1986 §7.3 SR3] an undeclared target parameter is a
    /// Syntax Rule violation, not a panic.
    #[test]
    fn undeclared_parameters_are_reported() {
        assert_eq!(declared_type(&Params::new(), "NOPE").err().map(|e| e.code), Some(Code::SyntaxRule));
    }

    /// Verifies: [DESIGN] a SET of a column the table does not have is
    /// reported, not a panic.
    #[test]
    fn a_candidate_row_needs_its_columns() {
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        let column = ColumnDesc {
            name: "K".into(),
            data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric },
            not_null: false,
            default: None,
            domain: None,
        };
        let desc = TableDesc {
            name: name.clone(),
            columns: vec![column],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        let rows = LogicalPlan::Scan { table: name.clone(), names: vec!["K".into()], needed: vec![0] };
        let write =
            binder::BoundWrite { named: name.clone(), table: name, desc, set: vec![(3, None)], rows, checks: Vec::new(), auth: "A".into() };
        assert_eq!(candidate(&write, &vec![Value::Null], ("§8.12 GR6", &NoSubqueries)).err().map(|e| e.code), Some(Code::Api));
    }
}
