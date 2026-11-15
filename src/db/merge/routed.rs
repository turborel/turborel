//! [ISO/IEC 9075-2:2023 §14.12 GR6a, §15.10, §15.13, §15.16] A MERGE into
//! a view whose INSTEAD OF triggers take some of its WHEN clauses: each
//! such clause's subject rows, or its candidate rows, given to the trigger
//! of its event in place of being written, in the order the clauses are
//! written; any other clause writes through the view as it does today.
//!
//! The trigger is the view's own or, through an updatable view, that of a
//! view under it (`triggers::instead_for`), and the MERGE is decided over
//! the base table's rows, each given to the trigger as the row of its view
//! that it is. A view that is not updatable is changed only through its
//! triggers, so every clause must be one they take [§14.12 SR2 to SR4]:
//! it is then decided over the view's own rows, read as its owner reads
//! them, each its ordinal as its row id (`held`).
//!
//! A candidate row for an insert a trigger takes is a row of its view:
//! each insert object column its value, every other column null, as an
//! INSERT into the view gives it (`triggers/rerouted.rs`). Nothing of a
//! base table is written for such a clause, so no BEFORE trigger, CHECK
//! OPTION or generated column of one is of it.

use super::super::triggers::{Event, Instead, instead_for, updatable, view_rows};
use super::super::write::{InsertRules, Target, object_columns};
use super::super::{Session, State, StatementContext, api};
use super::decide::{Merge, Planned, target_part};
use crate::ast::{MatchedAction, MergeInsert, MergeStatement, MergeWhen};
use crate::binder::{ObjectColumn, qualify};
use crate::catalog::{Actions, ColumnDesc, QualifiedName, applicable};
use crate::error::{Code, Result, SqlError, Subject};
use crate::exec::{HeldRows, Subqueries, TransitionRows};
use crate::storage::{Row, RowId, located};
use crate::value::datetime::Now;
use crate::value::{Value, assign};

/// [ISO/IEC 9075-2:2023 §14.12 GR6a ii 7 D] The rule the store assignment
/// of an insert value to a column of a trigger's view cites.
const STORED: &str = "ISO/IEC 9075-2:2023 §14.12 GR6a ii 7 D";

/// [ISO/IEC 9075-2:2023 §14.12 AR1b iii] The rule the INSERT privilege of
/// a WHEN NOT MATCHED clause a trigger takes cites.
const INSERT_PRIVILEGE: &str = "ISO/IEC 9075-2:2023 §14.12 AR1b iii";

/// Which INSTEAD OF trigger takes each WHEN clause of a MERGE, if one
/// does, and whether T is read as the view itself (`held`).
pub(super) struct Routing {
    /// Of each clause, in order, the trigger that takes it.
    pub(super) routes: Vec<Option<Instead>>,
    /// Whether T is a view that is not updatable, every clause taken by
    /// its own triggers.
    pub(super) held: bool,
}

/// [ISO/IEC 9075-2:2023 §14.12 SR2 to SR4] The routing of `stmt`, its
/// names in `schema`.
pub(super) fn routing(st: &State, stmt: &MergeStatement, schema: &str) -> Routing {
    let name = qualify(&stmt.target, schema);
    let routes: Vec<Option<Instead>> = stmt.clauses.iter().map(|c| instead_for(st, &name, event_of(c))).collect();
    // A view not updatable whose triggers take every clause; one they do
    // not is refused as T is bound, of the rule that clause breaks.
    let held = routes.iter().any(Option::is_some) && !updatable(st, &name) && routes.iter().all(Option::is_some);
    Routing { routes, held }
}

/// The trigger event of WHEN clause `c` [§4.46.2].
fn event_of(c: &MergeWhen) -> Event {
    match c {
        MergeWhen::NotMatched { .. } => Event::Insert,
        MergeWhen::Matched { action: MatchedAction::Update(_), .. } => Event::Update,
        MergeWhen::Matched { action: MatchedAction::Delete, .. } => Event::Delete,
    }
}

/// Where a WHEN NOT MATCHED clause's candidate rows go.
#[derive(Debug, Clone)]
pub(in crate::db) enum Inserting {
    /// Into a table, or through an updatable view into its base table.
    Table(Box<Target>),
    /// To the INSTEAD OF trigger that takes them, as rows of its view.
    Instead(ViewRows),
}

/// [ISO/IEC 9075-2:2023 §15.13 GR3a] How the insert values of a WHEN NOT
/// MATCHED clause make a row of the view of the trigger that takes it.
#[derive(Debug, Clone)]
pub(in crate::db) struct ViewRows {
    /// The insert object columns, as the binder takes them: a column of
    /// the base table, of a view updatable down to one, else of the view,
    /// with no default and no generator.
    objects: Vec<ObjectColumn>,
    /// The trigger's view's columns.
    columns: Vec<ColumnDesc>,
    /// Of each of them, the place among the insert values of the value it
    /// is given, if one is.
    given: Vec<Option<usize>>,
}

impl Inserting {
    /// The insert object columns, as the binder takes them.
    pub(in crate::db) fn objects(&self) -> Vec<ObjectColumn> {
        match self {
            Inserting::Table(t) => t.objects(),
            Inserting::Instead(v) => v.objects.clone(),
        }
    }

    /// How many insert values a candidate row is made of.
    pub(super) fn width(&self) -> usize {
        match self {
            Inserting::Table(t) => t.object().len(),
            Inserting::Instead(v) => v.objects.len(),
        }
    }

    /// The candidate row of the insert values `values`: a row of the table,
    /// as `Target::candidate` makes it, by `st`, warnings to `subqueries`;
    /// or a row of the trigger's view, each column given a value it, every
    /// other null.
    ///
    /// # Errors
    ///
    /// As `Target::candidate`; a value is not assignable to its column.
    pub(super) fn candidate(&self, st: &State, values: Row, subqueries: &dyn Subqueries) -> Result<Row> {
        let v = match self {
            Inserting::Table(t) => return t.candidate(st, values, subqueries),
            Inserting::Instead(v) => v,
        };
        v.columns
            .iter()
            .zip(&v.given)
            .map(|(c, at)| match at.and_then(|i| values.get(i)) {
                Some(value) => assign(value.clone(), &c.data_type, STORED),
                None => Ok(Value::Null),
            })
            .collect()
    }

    /// It, as a prepared statement's kept plan takes it again: a table's
    /// defaults those of a statement run under `auth` at `now`.
    ///
    /// # Errors
    ///
    /// As `Target::refreshed`.
    pub(super) fn refreshed(self, auth: &str, now: Now) -> Result<Inserting> {
        match self {
            Inserting::Table(t) => Ok(Inserting::Table(Box::new(t.refreshed(auth, now)?))),
            Inserting::Instead(v) => Ok(Inserting::Instead(v)),
        }
    }

    /// The table it inserts into, if it does.
    pub(super) fn table(&self) -> Option<&Target> {
        match self {
            Inserting::Table(t) => Some(t),
            Inserting::Instead(_) => None,
        }
    }
}

/// [ISO/IEC 9075-2:2023 §14.12 SR14, SR17, SR18a, AR1b iii] How the WHEN
/// NOT MATCHED clause `insert` of a MERGE into view `name` makes rows of
/// the view of `instead`, the trigger that takes it: its insert object
/// columns, each a column of `name`, none twice, INSERT on each held by
/// `user`. Of a view `held`, each is a column of the view, else of the
/// base table.
///
/// # Errors
///
/// SQLCODE -103 for a column `name` lacks or one listed twice; -104 for a
/// privilege `user` lacks.
pub(super) fn view_rows_of(
    st: &State,
    (name, insert, rules): (&QualifiedName, &MergeInsert, &InsertRules),
    (instead, held): (&Instead, bool),
    user: &str,
) -> Result<ViewRows> {
    let named = st.catalog.view(name).ok_or_else(|| api(format!("[DESIGN] {name} is no view of an INSTEAD OF trigger")))?;
    let listed = match insert.columns.as_deref() {
        None => (0..named.columns.len()).collect(),
        Some(list) => object_columns(name, &named.columns, list, rules)?,
    };
    let (holds, _) = applicable(&*st.catalog, user, name);
    let wanted = Actions { insert: listed.iter().copied().collect(), ..Actions::default() };
    let column_names: Vec<String> = named.columns.iter().map(|c| c.name.clone()).collect();
    if let Some(lacked) = holds.missing(&wanted, &column_names) {
        let msg = format!("[{INSERT_PRIVILEGE}] {user} lacks {lacked} on {name}");
        return Err(SqlError::new(Code::Privilege, msg).about(Subject::table(name.parts())));
    }
    let base = match (&instead.base, held) {
        (Some((table, columns)), false) => Some((st.base_table(table)?, columns)),
        (Some(_) | None, true) | (None, false) => None,
    };
    let mut objects = Vec::with_capacity(listed.len());
    for &k in &listed {
        let column = match (&base, instead.from_named.get(k)) {
            (Some((desc, columns)), Some(&j)) => columns.get(j).and_then(|&c| desc.columns.get(c)),
            (None, _) => named.columns.get(k),
            (Some(_), None) => None,
        };
        let desc = column.cloned().ok_or_else(|| api(format!("[DESIGN] column {k} of {name} is no column of the table under it")))?;
        objects.push(ObjectColumn { desc, default: Value::Null, draws: None });
    }
    let view = st.catalog.view(&instead.view).ok_or_else(|| api(format!("[DESIGN] {} is no view", instead.view)))?;
    let given = (0..view.columns.len()).map(|j| listed.iter().position(|&k| instead.from_named.get(k) == Some(&j))).collect();
    Ok(ViewRows { objects, columns: view.columns.clone(), given })
}

/// [ISO/IEC 9075-2:2023 §15.10 GR3a, §15.13 GR3a, §15.16 GR3a] The rows of
/// view `name`, T of a MERGE its triggers take whole, as its owner reads
/// them, each followed by its ordinal, its row id.
///
/// # Errors
///
/// The view's read fails; SQLCODE -910 past 2⁶⁴ rows.
pub(super) fn held_rows(cx: StatementContext<'_>, name: &QualifiedName) -> Result<TransitionRows> {
    let view = cx.st.catalog.view(name).cloned().ok_or_else(|| api(format!("[DESIGN] {name} is no view")))?;
    let mut held = HeldRows::new(cx.st.list_space());
    for (n, row) in view_rows(cx, &view)?.into_iter().enumerate() {
        let mut row = row?;
        row.push(located(RowId::try_from(n).map_err(|_| api("[DESIGN] a view has more rows than a row id counts"))?));
        held.push(row)?;
    }
    Ok(TransitionRows::with(held))
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §14.12 GR6a i 5 C II, GR6a i 7 B II, GR6a ii 8
    /// B] What clause `plan` of `merge` decided, given to `instead`, the
    /// trigger that takes it, in place of being written: each subject row
    /// as the row of its view it is, before and, of an update, after its
    /// SET clauses; each candidate row.
    ///
    /// # Errors
    ///
    /// A held row cannot be read back; the trigger's.
    pub(super) fn taken(&self, cx: StatementContext<'_>, merge: &Merge<'_>, instead: &Instead, plan: &mut Planned) -> Result<()> {
        let of = |row: &Row| if merge.held { Ok(row.clone()) } else { instead.of_base(row) };
        let mut transitions = HeldRows::new(cx.st.list_space());
        match plan {
            Planned::Updates(rows) => {
                rows.visit(&mut |(id, joined, new)| transitions.push((*id, of(&target_part(merge, joined)?)?, of(new)?)))?;
            }
            Planned::Deletes(rows) => {
                rows.visit(&mut |(id, joined)| transitions.push((*id, of(&target_part(merge, joined)?)?, Row::new())))?;
            }
            Planned::Inserts(_, rows) => rows.visit(&mut |row| transitions.push((0, Row::new(), row.clone())))?,
        }
        self.execute_trigger(cx, &instead.trigger, transitions)?;
        Ok(())
    }
}
