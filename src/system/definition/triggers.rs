//! [ISO/IEC 9075-11:2006 draft §6.53 to §6.58] The Definition Schema's
//! base tables of triggers: TRIGGERS, of each trigger descriptor [ISO/IEC
//! 9075-2:2023 §4.46.1]; TRIGGERED_UPDATE_COLUMNS, of each column of a
//! trigger column list; and TRIGGER_TABLE_USAGE, TRIGGER_COLUMN_USAGE,
//! TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE, of what each triggered
//! action names: the tables and views it reads, its triggered action column
//! set [§11.49 GR5i], its subject routines [§4.35.2], and the sequence
//! generators it draws from [§11.74 SR3b]. The views of triggers read them
//! (`system/information/triggers.rs`).
//!
//! CREATED, the timestamp of creation [§6.58 Description 13], is null: a
//! trigger keeps an order of creation, no timestamp (`TriggerDesc::created`).
//! ACTION_CONDITION and ACTION_STATEMENT are as written, or null if longer
//! than CHARACTER_DATA holds [§11.49 GR4], so ACTION_STATEMENT is not NOT
//! NULL, as the draft has it. ACTION_TIMING is INSTEAD OF too, of a trigger
//! on a view [§11.49 SR15].

use std::sync::Arc;

use super::{Columns, defined, ids, long};
use crate::catalog::{ActionTime, ColumnDesc, QualifiedName, TriggerDesc, TriggerEvent, TriggerLevel};
use crate::db::trigger_texts;
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, number, text};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-11:2006 draft §5.6] The precision of the domain
/// TIME_STAMP: TIMESTAMP(2) WITH TIME ZONE.
const TIME_STAMP_PRECISION: u32 = 2;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("TRIGGERS", triggers_columns, Rows::Fallible(triggers)),
        defined("TRIGGERED_UPDATE_COLUMNS", update_columns_columns, Rows::Infallible(update_columns)),
        defined("TRIGGER_TABLE_USAGE", table_usage_columns, Rows::Infallible(table_usage)),
        defined("TRIGGER_COLUMN_USAGE", column_usage_columns, Rows::Infallible(column_usage)),
        defined("TRIGGER_ROUTINE_USAGE", routine_usage_columns, Rows::Infallible(routine_usage)),
        defined("TRIGGER_SEQUENCE_USAGE", sequence_usage_columns, Rows::Infallible(sequence_usage)),
    ]
}

/// The columns naming a trigger, which each table's rows begin with.
fn trigger_named() -> Columns {
    Columns::of().ids("TRIGGER", true)
}

// ---------------------------------------------------------------------------
// [§6.58] TRIGGERS
// ---------------------------------------------------------------------------

fn triggers_columns() -> Vec<ColumnDesc> {
    let mut columns = trigger_named()
        .data("EVENT_MANIPULATION", true)
        .id("EVENT_OBJECT_CATALOG", true)
        .id("EVENT_OBJECT_SCHEMA", true)
        .id("EVENT_OBJECT_TABLE", true)
        .number("ACTION_ORDER", true)
        .data("ACTION_CONDITION", false)
        .data("ACTION_STATEMENT", false)
        .data("ACTION_ORIENTATION", true)
        .data("ACTION_TIMING", true)
        .id("ACTION_REFERENCE_OLD_TABLE", false)
        .id("ACTION_REFERENCE_NEW_TABLE", false)
        .id("ACTION_REFERENCE_OLD_ROW", false)
        .id("ACTION_REFERENCE_NEW_ROW", false)
        .done();
    columns.push(time_stamp("CREATED"));
    columns
}

/// [ISO/IEC 9075-11:2006 draft §5.6] A column `name` of TIME_STAMP, which
/// may be null.
fn time_stamp(name: &str) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Timestamp { precision: TIME_STAMP_PRECISION, zoned: true },
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }
}

/// Each trigger, in the order of creation.
fn triggers(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let all = seen.state.catalog.triggers();
    let mut rows = Vec::with_capacity(all.len());
    for t in &all {
        let (condition, statement) = trigger_texts(t)?;
        let names = &t.names;
        let referenced = [&names.old_table, &names.new_table, &names.old_row, &names.new_row];
        rows.push(
            [
                ids(&catalog, &t.name).to_vec(),
                vec![text(manipulation(&t.event))],
                ids(&catalog, &t.table).to_vec(),
                vec![number(action_order(&all, t)), condition.as_deref().map_or(Value::Null, long), long(&statement)],
                vec![text(orientation(t.level)), text(timing(t.time))],
                referenced.iter().map(|n| n.as_deref().map_or(Value::Null, text)).collect(),
                vec![Value::Null],
            ]
            .concat(),
        );
    }
    Ok(rows)
}

/// [§6.58 Description 2] EVENT_MANIPULATION of trigger event `e`.
fn manipulation(e: &TriggerEvent) -> &'static str {
    match e {
        TriggerEvent::Insert => "INSERT",
        TriggerEvent::Delete => "DELETE",
        TriggerEvent::Update(_) => "UPDATE",
    }
}

/// [§6.58 Description 12] ACTION_ORIENTATION of `level`.
fn orientation(level: TriggerLevel) -> &'static str {
    match level {
        TriggerLevel::Row => "ROW",
        TriggerLevel::Statement => "STATEMENT",
    }
}

/// [§6.58 Description 4] ACTION_TIMING of `time`.
fn timing(time: ActionTime) -> &'static str {
    match time {
        ActionTime::Before => "BEFORE",
        ActionTime::After => "AFTER",
        ActionTime::InsteadOf => "INSTEAD OF",
    }
}

/// [§6.58 Description 9] The ordinal position of `t` among the triggers of
/// `all` of its subject table, event, action time and orientation, which
/// execute in the order of creation (Feature T218).
fn action_order(all: &[&TriggerDesc], t: &TriggerDesc) -> usize {
    let alike = |o: &&&TriggerDesc| {
        o.table == t.table && manipulation(&o.event) == manipulation(&t.event) && o.time == t.time && o.level == t.level
    };
    all.iter().filter(alike).filter(|o| o.created <= t.created).count()
}

// ---------------------------------------------------------------------------
// [§6.53] TRIGGERED_UPDATE_COLUMNS
// ---------------------------------------------------------------------------

fn update_columns_columns() -> Vec<ColumnDesc> {
    trigger_named()
        .id("EVENT_OBJECT_CATALOG", true)
        .id("EVENT_OBJECT_SCHEMA", true)
        .id("EVENT_OBJECT_TABLE", true)
        .id("EVENT_OBJECT_COLUMN", true)
        .done()
}

/// [§6.53 Description 2] Each column of the trigger column list of an
/// UPDATE trigger that has one, in the order written.
fn update_columns(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.triggers() {
        let TriggerEvent::Update(columns) = &t.event else { continue };
        for c in columns {
            rows.push([ids(&catalog, &t.name).to_vec(), ids(&catalog, &t.table).to_vec(), vec![text(c)]].concat());
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// [§6.54 to §6.57] What each triggered action names
// ---------------------------------------------------------------------------

fn table_usage_columns() -> Vec<ColumnDesc> {
    trigger_named().id("TABLE_CATALOG", true).id("TABLE_SCHEMA", true).id("TABLE_NAME", true).done()
}

fn column_usage_columns() -> Vec<ColumnDesc> {
    trigger_named().id("TABLE_CATALOG", true).id("TABLE_SCHEMA", true).id("TABLE_NAME", true).id("COLUMN_NAME", true).done()
}

fn routine_usage_columns() -> Vec<ColumnDesc> {
    trigger_named().ids("SPECIFIC", true).done()
}

fn sequence_usage_columns() -> Vec<ColumnDesc> {
    trigger_named().ids("SEQUENCE", true).done()
}

/// A row of each trigger and each object `of` gives of it: the trigger's
/// name, then the object's.
fn named_by(seen: &Seen<'_>, of: impl Fn(&TriggerDesc) -> Vec<QualifiedName>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.triggers() {
        for object in of(t) {
            rows.push([ids(&catalog, &t.name), ids(&catalog, &object)].concat());
        }
    }
    rows
}

/// [§6.57] Each table and view a triggered action reads.
fn table_usage(seen: &Seen<'_>) -> Vec<Row> {
    named_by(seen, |t| t.reads.clone())
}

/// [§6.54] Each column of a triggered action column set.
fn column_usage(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.triggers() {
        for (table, column) in &t.columns {
            rows.push([ids(&catalog, &t.name).to_vec(), ids(&catalog, table).to_vec(), vec![text(column)]].concat());
        }
    }
    rows
}

/// [§6.55] Each subject routine of a triggered action, by its specific
/// name.
fn routine_usage(seen: &Seen<'_>) -> Vec<Row> {
    named_by(seen, |t| t.horizon.subjects().unwrap_or_default().to_vec())
}

/// [§6.56] Each sequence generator a triggered action draws from.
fn sequence_usage(seen: &Seen<'_>) -> Vec<Row> {
    named_by(seen, |t| t.sequences.clone())
}
