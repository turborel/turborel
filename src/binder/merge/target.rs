//! [ISO/IEC 9075-2:2023 §14.12 SR1 to SR4, SR7, SR8, SR10, AR1] The target
//! table of a MERGE: the table or updatable view it names, read as its base
//! table, the name it is exposed by, and the UPDATE and DELETE privileges
//! its WHEN clauses need. The INSERT privileges are the db layer's, which
//! checks each insert column list.

use super::super::from::FromItem;
use super::super::views::{WriteTarget, write_target};
use super::super::{Context, qualify, require};
use crate::ast::{MatchedAction, MergeStatement, MergeWhen};
use crate::catalog::{Actions, QualifiedName, TableDesc};
use crate::error::{At, Result};

/// The clause a MERGE's own rules are of, cited in full.
pub(super) const CLAUSE: &str = "ISO/IEC 9075-2:2023 §14.12";
/// [AR1b i] UPDATE on each update object column.
const UPDATE_PRIVILEGE: &str = "ISO/IEC 9075-2:2023 §14.12 AR1b i";
/// [AR1b ii] DELETE on T.
const DELETE_PRIVILEGE: &str = "ISO/IEC 9075-2:2023 §14.12 AR1b ii";

/// The target table, bound.
pub(super) struct Target {
    /// The table or view the statement names [SR1].
    pub(super) name: QualifiedName,
    /// It, read as its base table [SR7].
    pub(super) write: WriteTarget,
    /// Its FROM item, of the name it is exposed by [SR8], first in the
    /// rows the statement's conditions and SET clauses are bound over.
    pub(super) item: FromItem,
}

/// [SR1, SR2 to SR4, SR7, SR8, AR1] The target table of `stmt`: one that
/// can be changed as each of its WHEN clauses does, whose UPDATE and
/// DELETE privileges the current privileges hold. A view is changed as
/// ONLY of it [SR7], which is how `write_target` reads an updatable view,
/// its base table. Its exposed name is the merge correlation name if one
/// is written [SR8a], else its table name [SR8b]; its columns are every
/// column of T [SR10].
pub(super) fn bind_target(cx: Context<'_>, stmt: &MergeStatement) -> Result<Target> {
    let name = qualify(&stmt.target, cx.schema);
    let write = write_target(cx.db, &name, (&read_only_rule(stmt), cx.user), cx.now.noted()).or_at(stmt.target.at)?;
    required(cx, stmt, &write.named).or_at(stmt.target.at)?;
    let alone = FromItem::alone(name.clone(), stmt.target.at, (write.named.clone(), write.source.clone()));
    // A source row no target row matches is joined to nulls, so a NOT
    // NULL column of T may be null in the rows bound over.
    let item = FromItem { correlation: stmt.correlation.as_ref().map(|(c, _)| c.clone()), nullable: true, ..alone };
    Ok(Target { name, write, item })
}

/// [SR2, SR3, SR4] The rules a read-only T breaks: of each kind of WHEN
/// clause `stmt` has.
fn read_only_rule(stmt: &MergeStatement) -> String {
    let (mut insert, mut update, mut delete) = (false, false, false);
    for clause in &stmt.clauses {
        match clause {
            MergeWhen::NotMatched { .. } => insert = true,
            MergeWhen::Matched { action: MatchedAction::Update(_), .. } => update = true,
            MergeWhen::Matched { action: MatchedAction::Delete, .. } => delete = true,
        }
    }
    let rules: Vec<&str> = [(insert, "SR2"), (update, "SR3"), (delete, "SR4")].into_iter().filter(|(of, _)| *of).map(|(_, r)| r).collect();
    format!("{CLAUSE} {}", rules.join(", "))
}

/// [AR1b i, ii] That the current privileges hold UPDATE on each update
/// object column of `named`, the table or view named, of every WHEN
/// MATCHED clause that updates, and DELETE on it if one deletes. A column
/// it does not have is refused as its SET clause is bound.
fn required(cx: Context<'_>, stmt: &MergeStatement, named: &TableDesc) -> Result<()> {
    let (mut updates, mut deletes) = (false, false);
    let mut update = Actions::default();
    for clause in &stmt.clauses {
        match clause {
            MergeWhen::Matched { action: MatchedAction::Update(set), .. } => {
                updates = true;
                update.update.extend(set.iter().filter_map(|s| named.column(&s.column)));
            }
            MergeWhen::Matched { action: MatchedAction::Delete, .. } => deletes = true,
            MergeWhen::NotMatched { .. } => {}
        }
    }
    if updates {
        require(cx.db, cx.auth, named, &update, UPDATE_PRIVILEGE)?;
    }
    if deletes {
        require(cx.db, cx.auth, named, &Actions { delete: true, ..Actions::default() }, DELETE_PRIVILEGE)?;
    }
    Ok(())
}
