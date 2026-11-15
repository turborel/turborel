//! [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a i 1, GR6a ii 1 to 7] What a
//! MERGE does, decided from its joined rows before any row is changed:
//! each target row's subject clause, the one source row it matches by
//! that clause, and its candidate new row; each unmatched source row's
//! WHEN NOT MATCHED clause and candidate row. Held per clause, up to the
//! work area, the rest spilled (RULES.md D38).

use std::cmp::Ordering;

use super::super::assignment::Assigned;
use super::super::triggers::Instead;
use super::super::{State, StatementContext, api};
use super::routed::Inserting;
use crate::binder::{BoundAction, BoundMerge, BoundWhen};
use crate::error::{Code, Result, SqlError};
use crate::exec::{Drawn, HeldRows, Runner, eval, eval_cond, execute};
use crate::logical::{Expr, Truth};
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::{Row, RowId, row_id_of};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B III] The rule an update value's
/// assignment cites.
pub(super) const UPDATE_RULE: &str = "ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B III";

/// A MERGE as it runs: bound, where its WHEN NOT MATCHED clauses insert,
/// in order, what runs its subqueries, where a joined row holds its target
/// row's row id, the INSTEAD OF trigger that takes each clause, if one
/// does, and whether T is a view read as its rows (`merge/routed.rs`).
pub(super) struct Merge<'a> {
    pub(super) bound: &'a BoundMerge,
    pub(super) targets: &'a [Inserting],
    pub(super) runner: &'a Runner,
    pub(super) id_column: usize,
    pub(super) routes: &'a [Option<Instead>],
    pub(super) held: bool,
}

impl Merge<'_> {
    /// The INSTEAD OF trigger that takes clause `clause`, if one does.
    pub(super) fn route(&self, clause: usize) -> Option<&Instead> {
        self.routes.get(clause).and_then(Option::as_ref)
    }
}

/// What one clause does, decided: held up to the work area, the rest
/// spilled.
pub(super) enum Planned {
    /// A WHEN MATCHED clause's updates: each subject row's id, its row
    /// joined to its matching row, and its candidate new row.
    Updates(HeldRows<(RowId, Row, Row)>),
    /// A WHEN MATCHED clause's deletes: each subject row's id, and its row
    /// joined to its matching row.
    Deletes(HeldRows<(RowId, Row)>),
    /// A WHEN NOT MATCHED clause's candidate rows, and which of the
    /// MERGE's targets they go to.
    Inserts(usize, HeldRows<Row>),
}

impl Planned {
    /// Nothing yet of each of `clauses`, held in `space`.
    fn each(clauses: &[BoundWhen], space: &ListSpace) -> Vec<Planned> {
        let mut inserts = 0_usize;
        let mut planned = Vec::with_capacity(clauses.len());
        for clause in clauses {
            planned.push(match clause {
                BoundWhen::Matched { action: BoundAction::Update(_), .. } => Planned::Updates(HeldRows::new(space.clone())),
                BoundWhen::Matched { action: BoundAction::Delete, .. } => Planned::Deletes(HeldRows::new(space.clone())),
                BoundWhen::NotMatched { .. } => {
                    let target = inserts;
                    inserts = inserts.saturating_add(1);
                    Planned::Inserts(target, HeldRows::new(space.clone()))
                }
            });
        }
        planned
    }
}

/// What `merge` does to each row, from its joined rows, which `physical`
/// gives sorted by row id, and how many it read.
///
/// # Errors
///
/// A row cannot be read or held; a condition or value fails; a target row
/// has more than one matching row (SQLSTATE 21000).
pub(super) fn planned(
    StatementContext { st, tx }: StatementContext<'_>,
    merge: &Merge<'_>,
    physical: Plan,
) -> Result<(Vec<Planned>, Read)> {
    let mut planned = Planned::each(&merge.bound.clauses, &st.list_space());
    let mut grouping = Grouping::default();
    let mut read = Read::default();
    for row in execute(physical, st, tx.id) {
        let joined = row?;
        read.rows = read.rows.saturating_add(1);
        let id = joined.get(merge.id_column).ok_or_else(|| api("[DESIGN] a joined row of a MERGE has no row id"))?;
        let Some(id) = row_id_of(id)? else {
            unmatched(st, merge, &joined, &mut planned)?;
            continue;
        };
        read.matched = read.matched.saturating_add(1);
        let first = first_matched(merge, &joined, grouping.limit(id))?;
        if let Some(subject) = grouping.pair(id, first, joined)? {
            subject_row(merge, subject, &mut planned)?;
        }
    }
    if let Some(subject) = grouping.finish()? {
        subject_row(merge, subject, &mut planned)?;
    }
    Ok((planned, read))
}

/// The joined rows a MERGE read before any change: each a target row and
/// a source row ON matches it to, or a source row ON matches to none.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Read {
    /// The joined rows: those of `TR1 RIGHT OUTER JOIN TR2 ON SC`.
    pub(super) rows: usize,
    /// Those of a target row: those of `TR1, TR2 WHERE SC`.
    pub(super) matched: usize,
}

impl Read {
    /// [ISO/IEC 9075-2:2023 §23.1 GR2f ii] ROW_COUNT of a MERGE of
    /// `clauses`, the rows `SELECT COUNT(*)` counts of the target joined to
    /// the source before the MERGE: of WHEN MATCHED clauses alone, the
    /// pairs ON matches [1]; of WHEN NOT MATCHED clauses alone, the source
    /// rows it matches to none [2]; of both, every joined row [3].
    pub(super) fn row_count(self, clauses: &[BoundWhen]) -> usize {
        let matched = clauses.iter().any(|c| matches!(c, BoundWhen::Matched { .. }));
        let not_matched = clauses.iter().any(|c| matches!(c, BoundWhen::NotMatched { .. }));
        match (matched, not_matched) {
            (true, false) => self.matched,
            (false, true) => self.rows.saturating_sub(self.matched),
            // At least one clause is written [§14.12 Format], so no MERGE is
            // of neither.
            (true, true) | (false, false) => self.rows,
        }
    }
}

/// [GR6a i 1 A] The first WHEN MATCHED clause whose search condition, if
/// any, is true of `joined`, a target row joined to a source row ON
/// matches it to; none past clause `limit` tried, if given.
fn first_matched(merge: &Merge<'_>, joined: &Row, limit: Option<usize>) -> Result<Option<usize>> {
    for (j, clause) in merge.bound.clauses.iter().enumerate() {
        if limit.is_some_and(|l| j > l) {
            break;
        }
        let holds = match clause {
            BoundWhen::Matched { condition: None, .. } => true,
            BoundWhen::Matched { condition: Some(c), .. } => eval_cond(c, joined, merge.runner)? == Truth::True,
            BoundWhen::NotMatched { .. } => false,
        };
        if holds {
            return Ok(Some(j));
        }
    }
    Ok(None)
}

/// [GR6a i 1 B III, GR6a i 7] What the subject row `subject` is planned
/// for: its candidate new row, computed now, before any row is changed,
/// or its deletion.
fn subject_row(merge: &Merge<'_>, subject: Subject, planned: &mut [Planned]) -> Result<()> {
    let Subject { clause, id, joined } = subject;
    let bound = merge.bound;
    let unplanned = || api(format!("[DESIGN] clause {clause} of the MERGE is no WHEN MATCHED clause of its plan"));
    match planned.get_mut(clause).ok_or_else(unplanned)? {
        Planned::Updates(rows) => {
            let Some(BoundWhen::Matched { action: BoundAction::Update(set), .. }) = bound.clauses.get(clause) else {
                return Err(unplanned());
            };
            let old = target_part(merge, &joined)?;
            // [ISO/IEC 9075-2:2023 §15.16 GR3a] no base table written of a
            // clause a trigger takes, so no generated column of one.
            let generated: &[(usize, Expr)] = if merge.route(clause).is_some() { &[] } else { &bound.generated };
            let assigned = Assigned { desc: &bound.desc, set, generated };
            let new = assigned.candidate((&old, &joined), (UPDATE_RULE, merge.runner))?;
            rows.push((id, joined, new))
        }
        Planned::Deletes(rows) => rows.push((id, joined)),
        Planned::Inserts(..) => Err(unplanned()),
    }
}

/// The target row of `joined`: its first columns, those of T.
pub(super) fn target_part(merge: &Merge<'_>, joined: &Row) -> Result<Row> {
    let n = merge.bound.desc.columns.len();
    joined.get(..n).map(<[Value]>::to_vec).ok_or_else(|| api("[DESIGN] a joined row of a MERGE is shorter than its target's"))
}

/// [GR6a ii 4 to 7] The candidate row of `joined`, a source row no target
/// row matches, for the first WHEN NOT MATCHED clause that applies to it,
/// if one does.
fn unmatched(st: &State, merge: &Merge<'_>, joined: &Row, planned: &mut [Planned]) -> Result<()> {
    let Some(clause) = inserting(&merge.bound.clauses, joined) else { return Ok(()) };
    let (Some(BoundWhen::NotMatched { values, .. }), Some(Planned::Inserts(target, rows))) =
        (merge.bound.clauses.get(clause), planned.get_mut(clause))
    else {
        return Err(api(format!("[DESIGN] clause {clause} of the MERGE is no WHEN NOT MATCHED clause of its plan")));
    };
    let target = merge.targets.get(*target).ok_or_else(|| api("[DESIGN] a WHEN NOT MATCHED clause of a MERGE has no target"))?;
    // Bound in the order of the target's object columns [SR18a], one value each.
    if values.len() != target.width() {
        return Err(api("[DESIGN] a WHEN NOT MATCHED clause of a MERGE gives other values than its object columns"));
    }
    // [ISO/IEC 9075-2:2023 §4.29.1] the row's draws of one generator are one.
    let subqueries = &Drawn::new(merge.runner);
    let given = values.iter().map(|e: &Expr| eval(e, joined, subqueries)).collect::<Result<Row>>()?;
    rows.push(target.candidate(st, given, subqueries)?)
}

/// [GR6a ii 5] The first of `clauses` that is a WHEN NOT MATCHED clause
/// applying to `joined`, a source row no target row matches: one of no
/// search condition, or one whose flag in `joined` is true [GR6a ii 1 to
/// 3].
pub(super) fn inserting(clauses: &[BoundWhen], joined: &Row) -> Option<usize> {
    clauses.iter().position(|clause| match clause {
        BoundWhen::NotMatched { flag: None, .. } => true,
        BoundWhen::NotMatched { flag: Some(i), .. } => matches!(joined.get(*i), Some(Value::Bool(true))),
        BoundWhen::Matched { .. } => false,
    })
}

/// A subject row decided: its clause, its row id, and its row joined to
/// its matching row.
#[derive(Debug)]
pub(super) struct Subject {
    pub(super) clause: usize,
    pub(super) id: RowId,
    pub(super) joined: Row,
}

/// [GR6a i 1] The pairs of one target row after another, each target
/// row's pairs together: of the target row being read, the clause its
/// pairs choose so far.
#[derive(Default)]
pub(super) struct Grouping {
    group: Option<Group>,
}

/// One target row's pairs, so far: the first clause any is true of, the
/// first pair it is true of, and how many it is true of, counted to 2.
struct Group {
    id: RowId,
    chosen: Option<(usize, Row)>,
    matching: usize,
}

impl Grouping {
    /// The last clause a pair of target row `id` need be tried against:
    /// the clause its pairs so far chose, as no later one can be its
    /// subject clause; none, of a row none is chosen for yet.
    pub(super) fn limit(&self, id: RowId) -> Option<usize> {
        self.group.as_ref().filter(|g| g.id == id).and_then(|g| g.chosen.as_ref().map(|(c, _)| *c))
    }

    /// A pair of target row `id`, `joined`, the first clause true of it
    /// `first`: the subject row of the target row before, if this pair
    /// ends its pairs and it is one.
    ///
    /// # Errors
    ///
    /// As `finish`.
    pub(super) fn pair(&mut self, id: RowId, first: Option<usize>, joined: Row) -> Result<Option<Subject>> {
        let ended = match self.group.take() {
            Some(g) => {
                if g.id == id {
                    self.group = Some(g.with(first, joined));
                    return Ok(None);
                }
                g.subject()?
            }
            None => None,
        };
        self.group = Some(Group { id, chosen: None, matching: 0 }.with(first, joined));
        Ok(ended)
    }

    /// The subject row of the last target row, if it is one.
    ///
    /// # Errors
    ///
    /// [GR6a i 1 B II] SQLSTATE 21000 if its subject clause is true of
    /// more than one of its pairs.
    pub(super) fn finish(&mut self) -> Result<Option<Subject>> {
        self.group.take().map_or(Ok(None), Group::subject)
    }
}

impl Group {
    /// It, with a pair `joined` whose first clause true is `first`.
    fn with(self, first: Option<usize>, joined: Row) -> Group {
        let Some(f) = first else { return self };
        let order = self.chosen.as_ref().map_or(Ordering::Less, |(c, _)| f.cmp(c));
        match order {
            Ordering::Less => Group { chosen: Some((f, joined)), matching: 1, ..self },
            // Counted to 2: more than one is all the rule asks.
            Ordering::Equal => Group { matching: self.matching.saturating_add(1).min(MORE_THAN_ONE), ..self },
            Ordering::Greater => self,
        }
    }

    /// Its subject row, if a clause is true of a pair.
    fn subject(self) -> Result<Option<Subject>> {
        if self.matching > 1 {
            let clause = self.chosen.as_ref().map_or(0, |(c, _)| c.saturating_add(1));
            return Err(SqlError::new(
                Code::Cardinality,
                format!(
                    "[ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B II] a target row is matched by more than one source row its WHEN clause {clause} applies to"
                ),
            ));
        }
        Ok(self.chosen.map(|(clause, joined)| Subject { clause, id: self.id, joined }))
    }
}

/// [GR6a i 1 B II] A count of matching rows past which more do not matter.
const MORE_THAN_ONE: usize = 2;
