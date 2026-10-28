//! \[EXTENSION\] A session's diagnostics area [ISO/IEC 9075:1992 §4.19],
//! and GET DIAGNOSTICS, which reads it [§18.1].
//!
//! Every statement but GET DIAGNOSTICS empties the area as it begins, and
//! places its condition there as it ends: the one its SQLCODE and SQLSTATE
//! report, successful completion too [§4.18], so the area holds one
//! condition after each statement, none before the session's first. A
//! statement raises no other: MORE is always N. What the condition is
//! about, a constraint, table, column or cursor, is what its error names
//! (`error::Subject`).

use super::assignment::{assign_targets, declared_type};
use super::{Completion, Session, State, rule};
use crate::ast::{ConditionItem, GetDiagnostics, ParameterSpecification, StatementItem, ValueSpecification};
use crate::binder;
use crate::catalog::{QualifiedName, applicable};
use crate::error::{Code, Result, SqlState, Subject, fail_with};
use crate::params::Params;
use crate::settings::CATALOG_NAME;
use crate::value::{DataType, Decimal, Value};

/// \[DESIGN\] The naming authority of the conditions this implementation
/// defines, its SQLSTATE class TB [§18.1 GR3b, GR3c].
const IMPLEMENTATION: &str = "TURBOREL";
/// [ISO/IEC 9075:1992 §18.1 GR3b, GR3c] The naming authority of the
/// conditions the standard defines.
const STANDARD: &str = "ISO 9075";

/// \[DESIGN\] [ISO/IEC 9075:1992 §4.19] What the last statement but GET
/// DIAGNOSTICS raised.
#[derive(Debug, Clone, Default)]
pub(super) struct Area {
    /// [§18.1 GR1c, Table 22] The statement, as COMMAND_FUNCTION names it.
    command: &'static str,
    /// [§18.1 GR1d] The prepared statement it executed, as
    /// DYNAMIC_FUNCTION names it; empty for one not prepared.
    dynamic: &'static str,
    /// [§18.1 GR1e] The rows it inserted, updated or deleted; 0 for a
    /// statement of another kind.
    row_count: usize,
    /// Its condition, once it ended.
    condition: Option<Condition>,
}

/// A condition placed in the area.
#[derive(Debug, Clone)]
struct Condition {
    state: SqlState,
    /// MESSAGE_TEXT [§18.1 GR3k]: an error's message; empty for a
    /// completion.
    message: String,
    subject: Subject,
}

impl Session {
    /// [ISO/IEC 9075:1992 §4.19] The area emptied, as a statement other
    /// than GET DIAGNOSTICS begins: one `command` names, of the prepared
    /// statement `dynamic` names, if one.
    pub(super) fn emptied(&self, (command, dynamic): (&'static str, &'static str)) {
        self.inner.borrow_mut().area = Area { command, dynamic, ..Area::default() };
    }

    /// [ISO/IEC 9075:1992 §18.1 GR1e] The statement changed `rows` rows:
    /// its completion, no data if none [X3.135-1986 §7.3 GR3a iv].
    pub(super) fn affected(&self, rows: usize) -> Completion {
        self.inner.borrow_mut().area.row_count = rows;
        if rows == 0 { Completion::NoData } else { Completion::Success }
    }

    /// [ISO/IEC 9075:1992 §4.19] The condition of a statement's `outcome`
    /// placed in the area, a completion's SQLSTATE `state` gives: `outcome`.
    pub(super) fn diagnosed<T>(&self, outcome: Result<T>, state: impl FnOnce(&T) -> SqlState) -> Result<T> {
        let condition = match &outcome {
            Ok(done) => Condition { state: state(done), message: String::new(), subject: Subject::default() },
            Err(e) => Condition { state: e.sqlstate(), message: e.described(), subject: e.subject.as_deref().cloned().unwrap_or_default() },
        };
        self.inner.borrow_mut().area.condition = Some(condition);
        outcome
    }

    /// The condition in the area about cursor `name` [ISO/IEC 9075:1992
    /// §18.1 GR3e].
    pub(super) fn about_cursor(&self, name: &str) {
        if let Some(condition) = self.inner.borrow_mut().area.condition.as_mut() {
            condition.subject.cursor = Some(name.to_owned());
        }
    }

    /// [ISO/IEC 9075:1992 §18.1] GET DIAGNOSTICS `get`: what the area holds
    /// assigned to its targets; the area as it was.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a target is not of its item's type \[SR1\], or the
    /// condition number not exact with scale 0 \[SR2\]; -301, SQLSTATE
    /// 35000, if no condition has that number \[GR2\].
    pub(super) fn get_diagnostics(&self, st: &State, get: &GetDiagnostics, params: &mut Params) -> Result<Completion> {
        let area = self.inner.borrow().area.clone();
        let count = usize::from(area.condition.is_some());
        let (targets, values) = match get {
            GetDiagnostics::Statement(items) => {
                let value = |item: StatementItem| match item {
                    StatementItem::Number => number(count),
                    StatementItem::More => text("N"),
                    StatementItem::CommandFunction => text(area.command),
                    StatementItem::DynamicFunction => text(area.dynamic),
                    StatementItem::RowCount => number(area.row_count),
                };
                typed(items.iter().map(|(t, item)| (t, item.numeric(), value(*item))), params)?
            }
            GetDiagnostics::Condition { number: n, items } => {
                let n = condition_number(n, &self.auth, params)?;
                let Some(condition) = area.condition.as_ref().filter(|_| n == 1) else {
                    let msg = format!("[ISO/IEC 9075:1992 §18.1 GR2] there is no condition {n}: the area holds {count}");
                    return fail_with(Code::Data, SqlState::INVALID_CONDITION_NUMBER, msg);
                };
                let shown = self.shown(st, condition);
                let value = |item: ConditionItem| shown.value(item, (n, condition));
                typed(items.iter().map(|(t, item)| (t, item.numeric(), value(*item))), params)?
            }
        };
        // [X3.135-1986 §8.6 GR8-GR10; ISO/IEC 9075:1992 §9.1] as a FETCH assigns.
        assign_targets(&values, &targets, params, "§8.6 GR8", "§8.6 GR10")?;
        Ok(Completion::Success)
    }

    /// The names `condition` gives, as this session's authorization
    /// identifier may see them [ISO/IEC 9075:1992 §18.1 GR3f–GR3j].
    fn shown(&self, st: &State, condition: &Condition) -> Shown {
        let catalog = st.settings.get(CATALOG_NAME).to_string().trim_end().to_owned();
        // [GR3j] but for a syntax error or access rule violation, a table
        // on which the identifier holds no privilege, and a constraint of
        // one, are not named.
        let access = ["42", "2A", "37"].contains(&condition.state.class());
        let visible = |(schema, ident): &(String, String)| {
            let name = QualifiedName { auth: schema.clone(), ident: ident.clone() };
            access || !applicable(&*st.catalog, &self.auth, &name).0.is_empty()
        };
        let Subject { constraint, table, column, cursor } = &condition.subject;
        let table = table.clone().filter(visible);
        let constraint = constraint.clone().filter(|_| table.is_some());
        Shown { catalog, constraint, column: column.clone().filter(|_| table.is_some()), table, cursor: cursor.clone() }
    }
}

/// What a condition names, as GET DIAGNOSTICS gives it.
struct Shown {
    catalog: String,
    constraint: Option<(String, String)>,
    table: Option<(String, String)>,
    column: Option<String>,
    cursor: Option<String>,
}

impl Shown {
    /// [ISO/IEC 9075:1992 §18.1 GR3] The value of `item` for condition
    /// `n`, `condition`: a zero-length string for a name it has not
    /// \[GR3o\], and for a server's and connection's, there being no SQL
    /// connections \[GR3n\].
    fn value(&self, item: ConditionItem, (n, condition): (i128, &Condition)) -> Value {
        let catalog_of = |named: &Option<(String, String)>| text(if named.is_some() { &self.catalog } else { "" });
        let schema_of = |named: &Option<(String, String)>| text(named.as_ref().map_or("", |(s, _)| s));
        let name_of = |named: &Option<(String, String)>| text(named.as_ref().map_or("", |(_, i)| i));
        let origin = if condition.state.class() == "TB" { IMPLEMENTATION } else { STANDARD };
        match item {
            ConditionItem::ConditionNumber => Value::Exact(Decimal::new(n, 0)),
            ConditionItem::ReturnedSqlstate => text(condition.state.as_str()),
            ConditionItem::ClassOrigin | ConditionItem::SubclassOrigin => text(origin),
            ConditionItem::ServerName | ConditionItem::ConnectionName => text(""),
            ConditionItem::ConstraintCatalog => catalog_of(&self.constraint),
            ConditionItem::ConstraintSchema => schema_of(&self.constraint),
            ConditionItem::ConstraintName => name_of(&self.constraint),
            ConditionItem::CatalogName => catalog_of(&self.table),
            ConditionItem::SchemaName => schema_of(&self.table),
            ConditionItem::TableName => name_of(&self.table),
            ConditionItem::ColumnName => text(self.column.as_deref().unwrap_or("")),
            ConditionItem::CursorName => text(self.cursor.as_deref().unwrap_or("")),
            ConditionItem::MessageText => text(&condition.message),
            ConditionItem::MessageLength => number(condition.message.chars().count()),
            ConditionItem::MessageOctetLength => number(condition.message.len()),
        }
    }
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

fn number(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// [ISO/IEC 9075:1992 §18.1 SR1] The targets and their values, each
/// target of its item's data type: exact numeric with scale 0 if `numeric`,
/// else a character string (Table 21).
fn typed<'a>(
    items: impl Iterator<Item = (&'a ParameterSpecification, bool, Value)>,
    params: &Params,
) -> Result<(Vec<ParameterSpecification>, Vec<Value>)> {
    let (mut targets, mut values) = (Vec::new(), Vec::new());
    for (target, numeric, value) in items {
        let declared = declared_type(params, &target.name)?;
        let fits = if numeric { matches!(declared, DataType::Exact { scale: 0, .. }) } else { declared.is_character() };
        if !fits {
            let wanted = if numeric { "exact numeric with scale 0" } else { "a character string" };
            return Err(rule(format!("[ISO/IEC 9075:1992 §18.1 SR1] target {} is {declared}, not {wanted}", target.name)).or_at(target.at));
        }
        targets.push(target.clone());
        values.push(value);
    }
    Ok((targets, values))
}

/// [ISO/IEC 9075:1992 §18.1 SR2] The `<condition number>` `n`'s value.
fn condition_number(n: &ValueSpecification, auth: &str, params: &Params) -> Result<i128> {
    let not_exact = || rule("[ISO/IEC 9075:1992 §18.1 SR2] the <condition number> is exact numeric with scale 0".to_owned());
    let (value, data_type) = binder::value_specification(n, auth, params)?;
    if !matches!(data_type, DataType::Exact { scale: 0, .. }) {
        return Err(not_exact());
    }
    match value {
        Value::Exact(d) => Ok(d.units),
        Value::Null => fail_with(
            Code::Data,
            SqlState::INVALID_CONDITION_NUMBER,
            "[ISO/IEC 9075:1992 §18.1 GR2] the <condition number> is null".to_owned(),
        ),
        Value::Char(_) | Value::Approx(_) | Value::Extension(_) => Err(not_exact()),
    }
}
