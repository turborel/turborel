//! [ISO/IEC 9075-2:2023 §4.46.2] The state changes of a statement: for
//! each subject table and trigger event, the transitions the statement
//! made, each with the columns its update operation updated (OC), held as
//! lists that spill past MEMORY.WORK_BYTES (RULES.md D38).
//!
//! A state change is identified by its subject table, its trigger event
//! and a column list: of an UPDATE, the empty list and every set of the
//! table's columns that meets OC. A trigger of a trigger column list L is
//! activated by the state change whose column list is L, so the
//! transitions it sees are those whose OC meets L, or every one of its
//! event if L is empty. A state change arises as its statement decides
//! its rows, of no rows too, so a statement-level trigger is executed
//! for a statement that changes none.

use crate::catalog::{QualifiedName, TriggerDesc, TriggerEvent};
use crate::error::Result;
use crate::exec::HeldRows;
use crate::spill::ListSpace;
use crate::storage::{Row, RowId};

/// [ISO/IEC 9075-2:2023 §4.46.2] A trigger event, of a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::db) enum Event {
    /// INSERT.
    Insert,
    /// DELETE.
    Delete,
    /// UPDATE.
    Update,
}

/// The transitions of one subject table and trigger event that one update
/// operation made, of one OC: each its row's id, its old row and its new
/// row, an empty row for one it has not.
struct Segment {
    table: QualifiedName,
    event: Event,
    columns: Vec<String>,
    rows: HeldRows<(RowId, Row, Row)>,
}

/// [ISO/IEC 9075-2:2023 §4.46.2] The state changes of a statement
/// execution context, and the statement-level BEFORE triggers considered
/// executed for theirs.
pub(in crate::db) struct StateChanges {
    space: ListSpace,
    segments: Vec<Segment>,
    executed: Vec<QualifiedName>,
}

/// [ISO/IEC 9075-2:2023 §4.46.2] Whether trigger `t` is activated by the
/// state change of table `table`, event `event` and OC `columns`: its
/// subject table and event are theirs, and its trigger column list is
/// empty or meets `columns`.
pub(in crate::db) fn activates(t: &TriggerDesc, (table, event, columns): (&QualifiedName, Event, &[String])) -> bool {
    if t.table != *table {
        return false;
    }
    match (&t.event, event) {
        (TriggerEvent::Insert, Event::Insert) | (TriggerEvent::Delete, Event::Delete) => true,
        (TriggerEvent::Update(list), Event::Update) => list.is_empty() || list.iter().any(|c| columns.contains(c)),
        (TriggerEvent::Insert, Event::Delete | Event::Update)
        | (TriggerEvent::Delete, Event::Insert | Event::Update)
        | (TriggerEvent::Update(_), Event::Insert | Event::Delete) => false,
    }
}

impl StateChanges {
    /// None yet, their lists spilled in `space`.
    pub(in crate::db) fn new(space: ListSpace) -> StateChanges {
        StateChanges { space, segments: Vec::new(), executed: Vec::new() }
    }

    /// The state change of table `table`, event `event` and OC `columns`
    /// arises, if it has not.
    pub(in crate::db) fn arise(&mut self, (table, event, columns): (&QualifiedName, Event, &[String])) {
        let known = self.segments.iter().any(|s| s.table == *table && s.event == event && s.columns == columns);
        if !known {
            let rows = HeldRows::new(self.space.clone());
            self.segments.push(Segment { table: table.clone(), event, columns: columns.to_vec(), rows });
        }
    }

    /// Adds the transition `change` to the state change of table `table`,
    /// event `event` and OC `columns`, which arises if it has not.
    ///
    /// # Errors
    ///
    /// Its list's, spilled.
    pub(in crate::db) fn add(&mut self, of: (&QualifiedName, Event, &[String]), change: (RowId, Row, Row)) -> Result<()> {
        let (table, event, columns) = of;
        let last = self.segments.iter().rposition(|s| s.table == *table && s.event == event && s.columns == columns);
        let at = if let Some(at) = last {
            at
        } else {
            self.arise(of);
            self.segments.len().saturating_sub(1)
        };
        match self.segments.get_mut(at) {
            Some(s) => s.rows.push(change),
            None => Ok(()),
        }
    }

    /// Whether statement-level trigger `t` is considered executed already,
    /// noting it is from now on.
    pub(in crate::db) fn executed(&mut self, t: &TriggerDesc) -> bool {
        if self.executed.contains(&t.name) {
            return true;
        }
        self.executed.push(t.name.clone());
        false
    }

    /// Whether trigger `t` is activated by one of them.
    pub(in crate::db) fn activate(&self, t: &TriggerDesc) -> bool {
        self.segments.iter().any(|s| activates(t, (&s.table, s.event, &s.columns)))
    }

    /// The transitions of the state change that activates trigger `t`, in
    /// the order they were made (US048), into a list spilled in their space.
    ///
    /// # Errors
    ///
    /// Their lists'.
    pub(in crate::db) fn transitions(&mut self, t: &TriggerDesc) -> Result<HeldRows<(RowId, Row, Row)>> {
        let mut all = HeldRows::new(self.space.clone());
        for s in &mut self.segments {
            if activates(t, (&s.table, s.event, &s.columns)) {
                s.rows.visit(&mut |change| all.push(change.clone()))?;
            }
        }
        Ok(all)
    }
}
