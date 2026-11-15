//! The trigger descriptor [ISO/IEC 9075-2:2023 §4.46.1, §11.49 GR5].

use super::{Horizon, QualifiedName};
pub use crate::ast::{ActionTime, TriggerLevel};

/// [ISO/IEC 9075-2:2023 §4.46.1] A trigger event, its trigger column list
/// by column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerEvent {
    /// INSERT.
    Insert,
    /// DELETE.
    Delete,
    /// UPDATE, with its trigger column list: empty if none was written
    /// [§11.49 GR5h].
    Update(Vec<String>),
}

/// [ISO/IEC 9075-2:2023 §4.46.1] The old and new transition variable names
/// and the old and new transition table names a trigger descriptor
/// includes, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransitionNames {
    /// The old transition variable name.
    pub old_row: Option<String>,
    /// The new transition variable name.
    pub new_row: Option<String>,
    /// The old transition table name.
    pub old_table: Option<String>,
    /// The new transition table name.
    pub new_table: Option<String>,
}

/// [ISO/IEC 9075-2:2023 §4.46.1, §11.49 GR5] A trigger descriptor.
#[derive(Debug, Clone)]
pub struct TriggerDesc {
    /// The trigger's name \[GR5a\].
    pub name: QualifiedName,
    /// The name of its subject table, a base table or a view \[GR5b\].
    pub table: QualifiedName,
    /// Its trigger action time \[GR5c\].
    pub time: ActionTime,
    /// Whether it is row-level or statement-level \[GR5d\].
    pub level: TriggerLevel,
    /// Its trigger event, with its trigger column list \[GR5e, GR5h\].
    pub event: TriggerEvent,
    /// Its transition variable and table names \[GR5f\].
    pub names: TransitionNames,
    /// \[DESIGN\] Its `<trigger definition>` as written, which holds the
    /// triggered action \[GR5g\]: read again to bind the action, as a view's
    /// query is.
    pub text: String,
    /// The schema of the names in `text` written without one [ISO/IEC
    /// 9075:1992 §5.4 SR8]: the trigger's own, in a `<schema definition>`;
    /// else the session's default schema as it was defined.
    pub schema: String,
    /// The tables and views its triggered action names, its subject table
    /// only if the action names it: what the action references [§11.31
    /// SR7g, §11.33 SR4g].
    pub reads: Vec<QualifiedName>,
    /// The columns its triggered action sees of each base table it reads,
    /// and of its subject table, as they were when it was defined [§11.21
    /// GR2, Note 561].
    pub horizon: Horizon,
    /// The triggered action column set \[GR5i\]: every column its triggered
    /// action names, by its table and its name.
    pub columns: Vec<(QualifiedName, String)>,
    /// \[DESIGN\] Its timestamp of creation \[GR5j\], as an order: one more
    /// than the greatest of the triggers there were as it was created, so
    /// no two triggers of a catalog share one (IS003 none). Triggers
    /// execute oldest first (Feature T218).
    pub created: u64,
}

impl TriggerDesc {
    /// [ISO/IEC 9075-2:2023 §4.46.1] Whether it is an INSTEAD OF trigger.
    pub fn instead(&self) -> bool {
        self.time == ActionTime::InsteadOf
    }
}
