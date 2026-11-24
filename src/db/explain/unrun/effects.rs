//! \[EXTENSION\] What a data change EXPLAIN shows would do beside its rows
//! (RULES.md D983), read from the definitions alone: the indexes whose
//! entries it would write, the referential actions that would cascade
//! from it [ISO/IEC 9075-2:2023 §15.18], through the tables they write in
//! turn, and the triggers each change of a table would activate
//! [§4.46.2].

use std::fmt::Write;

use super::super::super::State;
use super::super::super::triggers::{Event, activates};
use crate::ast::ReferentialAction;
use crate::catalog::{ActionTime, ForeignKey, QualifiedName, TriggerDesc, TriggerEvent, TriggerLevel};

/// What a data change changes of a table itself.
pub(super) struct Changes {
    /// The base table.
    pub(super) table: QualifiedName,
    /// Each trigger event it makes of the table's rows, with the columns
    /// an UPDATE sets (OC) or an INSERT gives.
    pub(super) events: Vec<(Event, Vec<usize>)>,
}

/// A change of one table, of one trigger event, and the columns it sets.
type Change = (QualifiedName, Event, Vec<usize>);

/// Writes to `text` the lines of what `changes` would do beside its rows:
/// the indexes it maintains, the actions that cascade from it, and the
/// triggers that would fire, of its table and of those the actions write.
pub(super) fn shown(st: &State, changes: &Changes, text: &mut String) {
    let maintained = maintained(st, changes);
    let list = if maintained.is_empty() { "none".to_owned() } else { maintained.join(", ") };
    // Writing to a String cannot fail.
    let _ = writeln!(text, "-- maintains: {list}");
    let mut reached: Vec<Change> = changes.events.iter().map(|(e, c)| (changes.table.clone(), *e, c.clone())).collect();
    let mut at = 0_usize;
    while let Some(change) = reached.get(at).cloned() {
        at = at.saturating_add(1);
        for (line, next) in cascaded(st, &change) {
            let _ = writeln!(text, "{line}");
            if !reached.contains(&next) {
                reached.push(next);
            }
        }
    }
    let mut fired: Vec<&QualifiedName> = Vec::new();
    for (table, event, columns) in &reached {
        for t in activated(st, (table, *event, columns)) {
            if !fired.contains(&&t.name) {
                fired.push(&t.name);
                let _ = writeln!(text, "{}", trigger_line(t));
            }
        }
    }
}

/// The indexes of the table of `changes` whose entries it would write,
/// each as `NAME [method]`: every one of an INSERT or a DELETE; of an
/// UPDATE, each whose key reads a column it sets, every global one, and
/// every one if a BEFORE row trigger may set any column (RULES.md D875).
fn maintained(st: &State, changes: &Changes) -> Vec<String> {
    let table = &changes.table;
    let before = changes.events.iter().any(|(e, c)| {
        *e == Event::Update && activated(st, (table, *e, c)).iter().any(|t| matches!((t.time, t.level), (ActionTime::Before, TriggerLevel::Row)))
    });
    let writes = |reads: &[usize], global: bool| {
        changes.events.iter().any(|(e, c)| *e != Event::Update || global || before || reads.iter().any(|r| c.contains(r)))
    };
    st.table_indexes(table)
        .into_iter()
        .filter(|(ix, partition)| writes(&ix.reads(), partition.is_some()))
        .map(|(ix, _)| format!("{} [{}]", ix.name, ix.method))
        .collect()
}

/// The referential actions a change `change` makes of the tables whose
/// foreign keys reference its table, each its line and the change it
/// makes in turn [ISO/IEC 9075-2:2023 §15.18 GR9, GR10]: of a DELETE,
/// CASCADE deletes, SET NULL and SET DEFAULT update the referencing
/// columns; of an UPDATE of a referenced column, each updates them.
fn cascaded(st: &State, (table, event, columns): &Change) -> Vec<(String, Change)> {
    let mut found = Vec::new();
    for child in st.catalog.tables() {
        for (name, fk) in child.foreign_keys().filter(|(_, fk)| fk.table == *table) {
            let Some((verb, action)) = acted(fk, *event, columns) else { continue };
            let Some(word) = writes(action) else { continue };
            let deletes = matches!((event, action), (Event::Delete, ReferentialAction::Cascade));
            let next = if deletes { (child.name.clone(), Event::Delete, Vec::new()) } else { (child.name.clone(), Event::Update, fk.columns.clone()) };
            found.push((format!("-- cascades: ON {verb} {word} to {} by {name}", child.name), next));
        }
    }
    found
}

/// The rule `fk` follows of a change of `event` of the columns `columns`
/// of the table it references, and its event's word: its delete rule of
/// a DELETE, its update rule of an UPDATE of a referenced column, none
/// else.
fn acted(fk: &ForeignKey, event: Event, columns: &[usize]) -> Option<(&'static str, ReferentialAction)> {
    match event {
        Event::Delete => Some(("DELETE", fk.on_delete)),
        Event::Update => fk.referenced.iter().any(|c| columns.contains(c)).then_some(("UPDATE", fk.on_update)),
        Event::Insert => None,
    }
}

/// The words of `action`, if it writes the referencing rows.
fn writes(action: ReferentialAction) -> Option<&'static str> {
    match action {
        ReferentialAction::Cascade => Some("CASCADE"),
        ReferentialAction::SetNull => Some("SET NULL"),
        ReferentialAction::SetDefault => Some("SET DEFAULT"),
        ReferentialAction::NoAction | ReferentialAction::Restrict => None,
    }
}

/// The triggers, but INSTEAD OF ones, a change of `event` of the columns
/// `columns` of `table` activates [ISO/IEC 9075-2:2023 §4.46.2].
fn activated<'s>(st: &'s State, (table, event, columns): (&QualifiedName, Event, &[usize])) -> Vec<&'s TriggerDesc> {
    if !st.catalog.any_triggers() {
        return Vec::new();
    }
    let desc = st.catalog.table(table);
    let names: Vec<String> = columns.iter().filter_map(|&c| desc.and_then(|d| d.columns.get(c)).map(|col| col.name.clone())).collect();
    st.catalog.triggers_on(table).into_iter().filter(|t| !t.instead() && activates(t, (table, event, &names))).collect()
}

/// The line EXPLAIN shows of trigger `t`: its name, action time, event,
/// level and subject table.
pub(super) fn trigger_line(t: &TriggerDesc) -> String {
    let time = match t.time {
        ActionTime::Before => "BEFORE",
        ActionTime::After => "AFTER",
        ActionTime::InsteadOf => "INSTEAD OF",
    };
    let event = match &t.event {
        TriggerEvent::Insert => "INSERT".to_owned(),
        TriggerEvent::Delete => "DELETE".to_owned(),
        TriggerEvent::Update(list) => {
            if list.is_empty() {
                "UPDATE".to_owned()
            } else {
                format!("UPDATE OF {}", list.join(", "))
            }
        }
    };
    let level = match t.level {
        TriggerLevel::Row => "ROW",
        TriggerLevel::Statement => "STATEMENT",
    };
    format!("-- trigger: {}, {time} {event} FOR EACH {level} on {}", t.name, t.table)
}
