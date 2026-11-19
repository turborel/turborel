//! [ISO/IEC 9075-2:2023 §7.6 SR11, GR5] A `<data change delta table>` in
//! a `<table reference>`: the rows its statement gave, run before the query
//! is bound, of the subject table's columns (RULES.md D183).

use super::from::{Bounds, FromItem};
use super::qualify;
use super::views::Source;
use crate::ast::{DeltaTable, Given};
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::{Code, Result, SqlError};
use crate::exec::TransitionRows;
use crate::logical::LogicalPlan;

/// The rows given `d`; or, of one its statement runs for, but not yet,
/// none, of its subject table's columns, so that a statement prepared is
/// described.
fn given(b: &Bounds<'_>, d: &DeltaTable) -> Result<Given> {
    if let Some(given) = d.rows.given() {
        return Ok(given);
    }
    let subject = d.subject().filter(|_| d.rows.admitted()).ok_or_else(|| {
        SqlError::new(
            Code::Unsupported,
            "[EXTENSION] a <data change delta table> stands only as the one <table reference> of the FROM clause of a query a cursor opens, directly, prepared or declared (RULES.md D183)",
        )
    })?;
    let name = qualify(subject, b.cx.schema);
    let desc = b.cx.db.catalog.table(&name).ok_or_else(|| {
        SqlError::new(
            Code::Unsupported,
            format!("[EXTENSION] a <data change delta table> of {name}, not a base table, is not supported (RULES.md D183)"),
        )
    })?;
    Ok((TransitionRows::new(b.cx.db.list_space()), name, desc.columns.clone()))
}

/// The delta table `d`, its first column at `offset` of the FROM row.
///
/// # Errors
///
/// SQLCODE -900 if it stands where no delta table's statement runs, which
/// is anywhere but as the one table reference of the FROM clause of a
/// cursor's query or a SELECT INTO; -103 if its derived column list
/// names other than one column of each of the subject table's.
pub(super) fn delta_item(b: &Bounds<'_>, d: &DeltaTable, offset: usize) -> Result<FromItem> {
    let (rows, table, mut columns) = given(b, d)?;
    if let Some(names) = &d.columns {
        if names.len() != columns.len() {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §7.6 SR12b ii 1] the derived column list names {} columns of {}", names.len(), columns.len());
            return Err(SqlError::new(Code::SyntaxRule, msg));
        }
        for (c, name) in columns.iter_mut().zip(names) {
            c.name.clone_from(name);
        }
    }
    // As a table of no constraints, since what a column is drawn from is
    // no column of the subject table.
    for c in &mut columns {
        (c.default, c.generated, c.domain) = (None, None, None);
    }
    let ident = d.correlation.clone().unwrap_or_else(|| table.ident.clone());
    let name = QualifiedName { auth: String::new(), ident: ident.clone() };
    let names = columns.iter().map(|c| c.name.clone()).collect();
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    Ok(FromItem {
        table: name,
        at: d.at,
        correlation: Some(ident),
        desc,
        offset,
        source: Source::Derived(LogicalPlan::Transition { rows, names }),
        nullable: false,
        common: Vec::new(),
        drawn: None,
        family: Vec::new(),
    })
}
