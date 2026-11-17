//! Application-time periods [ISO/IEC 9075-2:2023 §4.16, §11.3, §11.27]: a
//! period's name and columns judged, as CREATE TABLE defines one or ALTER
//! TABLE adds one, and its period constraint, CHECK (start < end), named.
//! A system-time period is of a system-versioned table (Feature T180),
//! which is not yet, and is refused (RULES.md D18).

use super::{Described, Pending, Within};
use crate::ast::ReferentialAction;
use crate::ast::{PeriodDefinition, PeriodName, TableConstraint, TableDefinition, TableElement};
use crate::catalog::{ConstraintDesc, ConstraintKind, PeriodDesc, QualifiedName, TableDesc};
use crate::db::rule;
use crate::error::{At, Code, Result, SqlError};
use crate::parser::period_constraint;
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B] The last part of a period
/// constraint's name (UV100).
const PERIOD: &str = "PERIOD";

/// Which statement defines a period, whose rules judge its columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::db) enum Defining {
    /// CREATE TABLE [§11.3 SR2].
    Created,
    /// ALTER TABLE ADD PERIOD [§11.27 SR5].
    Added,
}

impl Defining {
    /// The rule of CREATE TABLE `created`, or of ALTER TABLE ADD PERIOD
    /// `added`, cited.
    fn cite(self, created: &str, added: &str) -> String {
        match self {
            Defining::Created => format!("§11.3 {created}"),
            Defining::Added => format!("§11.27 {added}"),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §4.16.1] The application-time period name of
/// `name`.
///
/// # Errors
///
/// SQLCODE -900 if it is SYSTEM_TIME, a system-time period, of a
/// system-versioned table, which is not yet (RULES.md D18).
pub(in crate::db) fn application(name: &PeriodName) -> Result<&str> {
    match name {
        PeriodName::Application(n) => Ok(n),
        PeriodName::SystemTime => Err(SqlError::new(
            Code::Unsupported,
            "[DESIGN] PERIOD FOR SYSTEM_TIME is the period of a system-versioned table (Feature T180), not yet supported (RULES.md D18)",
        )),
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR1, SR2a] The `<table period definition>`
/// of `t`, if it has one. [SR2b] A typed table's `<typed table element
/// list>` has none to write (`parser/structured/typed.rs`).
///
/// # Errors
///
/// SQLCODE -103 if it has more than one application-time period \[SR1\],
/// or is temporary \[SR2a\]; -900 of a system-time period.
pub(in crate::db) fn written_period(t: &TableDefinition) -> Result<Option<&PeriodDefinition>> {
    let mut periods = t.elements.iter().filter_map(|e| if let TableElement::Period(p) = e { Some(p) } else { None });
    let Some(first) = periods.next() else { return Ok(None) };
    application(&first.name)?;
    if let Some(second) = periods.next() {
        application(&second.name)?;
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR1] table {} defines more than one application-time period", t.name.ident)));
    }
    if t.temporary.is_some() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR2a] temporary table {} has no period", t.name.ident)));
    }
    Ok(Some(first))
}

/// [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B, GR4e] Of table `desc`, defined
/// by `t` `within`, its names of schema `names`, where `taken` are the
/// constraint names there are besides its own: its application-time
/// period's descriptor and period constraint, CHECK (start < end), if it
/// defines one; then its keys WITHOUT OVERLAPS judged (`keyed_periods`).
///
/// # Errors
///
/// SQLCODE -103 for a rule of the period broken (`period_of`).
pub(in crate::db) fn define_period(
    (t, names, within): (&TableDefinition, &str, Within),
    desc: &mut TableDesc,
    taken: &[QualifiedName],
) -> Result<()> {
    let Some(def) = written_period(t)? else { return keyed_periods(desc) };
    let name = application(&def.name)?;
    let mut all = taken.to_vec();
    all.extend(desc.constraints.iter().map(|c| c.name.clone()));
    let constraint = constraint_name(&desc.name, name, &all);
    let period = period_of(desc, def, constraint.clone(), Defining::Created).or_at(def.at)?;
    let (table, columns) = (desc.name.clone(), desc.columns.clone());
    let mut d = Described::new((&table, names, within), &columns, all);
    d.table_constraint(&period_check(def, &constraint)?)?;
    desc.constraints.extend(d.constraints);
    desc.periods.push(period);
    keyed_periods(desc)
}

/// [ISO/IEC 9075-2:2023 §11.7 SR3b] Each unique constraint of `desc`
/// WITHOUT OVERLAPS of a period of it.
///
/// # Errors
///
/// SQLCODE -103 of one of a period it has not.
pub(in crate::db) fn keyed_periods(desc: &TableDesc) -> Result<()> {
    for c in &desc.constraints {
        let ConstraintKind::Unique { period: Some(period), .. } = &c.kind else { continue };
        if !desc.periods.iter().any(|p| p.name == *period) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.7 SR3b] {} has no period {period} for constraint {} to be WITHOUT OVERLAPS of",
                desc.name, c.name
            )));
        }
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B, §11.27 GR2a] The name of the
/// period constraint of period `period` of `table`, UV100: the table's
/// name, the period's and PERIOD, joined by underscores, then numbered if
/// a constraint of schema `table.auth` of `taken` has it.
pub(in crate::db) fn constraint_name(table: &QualifiedName, period: &str, taken: &[QualifiedName]) -> QualifiedName {
    let base = format!("{}_{period}_{PERIOD}", table.ident);
    let named = |ident: String| QualifiedName { auth: table.auth.clone(), ident };
    let free = |n: &QualifiedName| !taken.contains(n);
    let first = named(base.clone());
    if free(&first) {
        return first;
    }
    // Of one more names than `taken` has, one is free.
    (1..=taken.len().saturating_add(1)).map(|n| named(format!("{base}_{n}"))).find(free).unwrap_or(first)
}

/// [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B, §11.27 GR2b] The period
/// constraint of `def`, named `name`: CHECK (start < end), written where
/// the period is.
///
/// # Errors
///
/// None in practice: its text is made of delimited names.
pub(in crate::db) fn period_check(def: &PeriodDefinition, name: &QualifiedName) -> Result<TableConstraint> {
    let mut check = period_constraint(&name.ident, (&def.start.0, &def.end.0))?;
    check.at = def.at;
    if let Some(n) = check.name.as_mut() {
        n.auth = Some(name.auth.clone());
    }
    Ok(check)
}

/// [ISO/IEC 9075-2:2023 §4.15.6, §11.3 SR2e iv] Whether the column at `at`
/// of `desc` is known not nullable by a constraint NOT DEFERRABLE: NOT
/// NULL, or a PRIMARY KEY it is of.
pub(in crate::db) fn not_nullable(desc: &TableDesc, at: usize) -> bool {
    desc.constraints.iter().any(|c| {
        let of = match &c.kind {
            ConstraintKind::NotNull(column) => *column == at,
            ConstraintKind::Unique { columns, primary: true, .. } => columns.contains(&at),
            ConstraintKind::Unique { primary: false, .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => false,
        };
        of && !c.attributes.deferrable
    })
}

/// Whether `t` is DATE or a timestamp type [§11.3 SR2e iii].
fn datetime_bound(t: &DataType) -> bool {
    matches!(t, DataType::Date | DataType::Timestamp { .. })
}

/// [ISO/IEC 9075-2:2023 §11.3 SR2d, SR2e; §11.27 SR5c, SR5e to SR5i] The
/// descriptor of the period `def` of `desc`, defined as `defining` says,
/// its period constraint named `constraint`: its start and end columns two
/// of the table's, of one type, DATE or a timestamp type, written so if
/// CREATE TABLE defines it, each known not nullable, neither an identity
/// nor a generated column; and no column named as the period.
///
/// # Errors
///
/// SQLCODE -103 for each rule broken.
pub(in crate::db) fn period_of(
    desc: &TableDesc,
    def: &PeriodDefinition,
    constraint: QualifiedName,
    defining: Defining,
) -> Result<PeriodDesc> {
    let name = application(&def.name)?;
    let table = &desc.name;
    if desc.column(name).is_some() {
        let cited = defining.cite("SR2d", "SR5c");
        return Err(rule(format!("[ISO/IEC 9075-2:2023 {cited}] table {table} has a column named as period {name}")));
    }
    let column = |(c, at): &(String, usize), (created, added): (&str, &str)| {
        desc.column_desc(c).ok_or_else(|| {
            let cited = defining.cite(created, added);
            rule(format!("[ISO/IEC 9075-2:2023 {cited}] table {table} has no column {c} to bound period {name}")).or_at(*at)
        })
    };
    let (start_at, start) = column(&def.start, ("SR2e i", "SR5e"))?;
    let (end_at, end) = column(&def.end, ("SR2e ii", "SR5f"))?;
    if start_at == end_at {
        let cited = defining.cite("SR2e", "SR5e, SR5f");
        return Err(
            rule(format!("[ISO/IEC 9075-2:2023 {cited}] period {name} starts and ends at one column, {}", start.name)).or_at(def.end.1)
        );
    }
    // [§11.3 SR2e iii] CREATE TABLE judges the <data type or domain name>
    // written, which a domain's name is not DATE or a timestamp type;
    // [§11.27 SR5g] ALTER TABLE, the declared types.
    let written = defining == Defining::Added || (start.domain.is_none() && end.domain.is_none());
    if !written || !datetime_bound(&start.data_type) || start.data_type != end.data_type {
        let cited = defining.cite("SR2e iii", "SR5g");
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 {cited}] the columns of period {name} are of one type, DATE or a timestamp type, not {} and {}",
            start.data_type, end.data_type
        ))
        .or_at(def.start.1));
    }
    for (at, c, place) in [(start_at, start, def.start.1), (end_at, end, def.end.1)] {
        if !not_nullable(desc, at) {
            let cited = defining.cite("SR2e iv", "SR5h");
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {cited}] column {} of period {name} is not NOT NULL NOT DEFERRABLE", c.name))
                .or_at(place));
        }
        if c.generated.is_some() {
            let cited = defining.cite("SR2e v 2 A", "SR5i");
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {cited}] column {} of period {name} is an identity or a generated column",
                c.name
            ))
            .or_at(place));
        }
    }
    Ok(PeriodDesc { name: name.to_owned(), start: start.name.clone(), end: end.name.clone(), constraint })
}

/// [ISO/IEC 9075-2:2023 §11.23 SR6] Whether column `column` of `desc` may
/// be dropped: no period of it starts or ends there.
///
/// # Errors
///
/// SQLCODE -103 if one does.
pub(in crate::db) fn unbounding(desc: &TableDesc, column: &str) -> Result<()> {
    match desc.periods.iter().find(|p| p.bounds(column)) {
        Some(p) => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.23 SR6] column {column} of {} bounds period {}", desc.name, p.name))),
        None => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.26 SR10] The constraint at `at` of `desc`,
/// removed from it, if it may be dropped: it is no period's constraint
/// \[SR10a\], and each period's columns are known not nullable still
/// \[SR10b\].
///
/// # Errors
///
/// SQLCODE -103 if not.
pub(in crate::db) fn removed_constraint(desc: &mut TableDesc, at: usize) -> Result<ConstraintDesc> {
    let removed = desc.constraints.remove(at);
    let dropped = &removed.name;
    for p in &desc.periods {
        if p.constraint == *dropped {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.26 SR10a] {dropped} is the constraint of period {} of {}, dropped with the period",
                p.name, desc.name
            )));
        }
        for column in [&p.start, &p.end] {
            if desc.column(column).is_some_and(|c| !not_nullable(desc, c)) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.26 SR10b] dropping {dropped} makes column {column} of period {} of {} possibly nullable",
                    p.name, desc.name
                )));
            }
        }
    }
    Ok(removed)
}

/// [ISO/IEC 9075-2:2023 §11.8 SR2, SR18a i] The periods of the foreign key
/// `p`, named `name`, of `child`, referencing table `target`, `parent`, if
/// temporal: the referencing period, then the referenced one.
///
/// # Errors
///
/// SQLCODE -103 if one is written without the other, or the referenced
/// columns are not \[SR2a\]; if it writes CASCADE, SET NULL or SET
/// DEFAULT \[SR2b\]; if either table lacks its period \[SR18a i 4, 5\];
/// or the periods are not of one type \[SR18a i 8\].
pub(in crate::db) fn temporal_reference(
    (child, parent): (&TableDesc, &TableDesc),
    p: &Pending,
    (name, target): (&QualifiedName, &QualifiedName),
) -> Result<Option<(String, String)>> {
    let r = &p.references;
    let (own, theirs) = match (&p.period, &r.period, &r.columns) {
        (None, None, _) => return Ok(None),
        (Some(own), Some(theirs), Some(_)) => (own, theirs),
        (Some(_) | None, _, _) => {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.8 SR2a] FOREIGN KEY {name} writes a PERIOD of its own columns and of those referenced, or neither"
            )));
        }
    };
    if [r.on_update, r.on_delete].into_iter().any(ReferentialAction::writes) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.8 SR2b] FOREIGN KEY {name} of a period has no action CASCADE, SET NULL or SET DEFAULT"
        )));
    }
    let bounds = |t: &TableDesc, period: &str, cited: &str| {
        t.period_columns(period).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.8 {cited}] {} has no period {period}", t.name)))
    };
    let [theirs_start, _] = bounds(parent, theirs, "SR18a i 4")?;
    let [own_start, _] = bounds(child, own, "SR18a i 5")?;
    let of = |t: &TableDesc, at: usize| t.columns.get(at).map(|c| c.data_type.clone());
    if of(parent, theirs_start) != of(child, own_start) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.8 SR18a i 8, §9.30] period {own} of {} and period {theirs} of {target} are not of one type",
            child.name
        )));
    }
    Ok(Some((own.clone(), theirs.clone())))
}
