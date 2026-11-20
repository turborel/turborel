//! ALTER COLUMN … SET DATA TYPE [ISO/IEC 9075-2:2023 §11.19], Feature
//! F382 (RULES.md D401).
//!
//! The column is given a predefined type of its own type's kind, as long
//! or as precise at least [SR13 to SR15] (`retype/widening.rs`), so each
//! value it holds is one of the new type. Nothing may refer to it whose
//! meaning a new type could change [SR6 to SR10]: a CHECK reading other
//! columns too, or another table's, an assertion, a trigger, a routine, a
//! view or a generation expression. A CHECK of the table that reads the
//! column alone is bound again of it.
//!
//! Its descriptor's type is replaced [GR1], and, as every value is stored
//! of its column's type, the table is given a new generation of its
//! storage, in either engine, its rows copied there each with the value
//! of the column assigned to the new type (`Reshape::Retype`, ISO/IEC
//! 9075:1992 §9.2), as ADD COLUMN copies them: by the transaction's own
//! writes, logged and undone with it, a row at a time. Its indexes are
//! made again on the new generation, of the new key type.

mod widening;

use super::{Made, Reshape};
use crate::ast::{AlterAction, TypeName};
use crate::binder::{generations, qualify, view_uses};
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc, TriggerEvent, owner_of, sole};
use crate::db::ddl::Dropped;
use crate::db::{State, rule};
use crate::error::Result;
use crate::logical::expr_columns;
use crate::value::DataType;

/// The column `column` of table `table`, as a use names it.
type Use = (QualifiedName, String);

/// [§11.19 SR3 to SR5] That the column at `at` of `desc` is neither its
/// self-referencing column nor the start or end of a period; and,
/// \[DESIGN\], of no domain, whose type it takes.
///
/// # Errors
///
/// SQLCODE -103 if it is.
fn retypable(desc: &TableDesc, at: usize) -> Result<()> {
    let qn = &desc.name;
    let Some(c) = desc.columns.get(at) else { return Ok(()) };
    if desc.typed.as_ref().is_some_and(|t| t.self_referencing == c.name) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.19 SR3] column {} of {qn} is its self-referencing column", c.name)));
    }
    if let Some((period, system)) = super::nullability::bounded_period(desc, at) {
        let clause = if system { "SR4" } else { "SR5" };
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.19 {clause}] column {} of {qn} bounds the period {period}", c.name)));
    }
    if let Some(domain) = &c.domain {
        return Err(rule(format!(
            "[DESIGN] column {} of {qn} is of domain {}, whose data type it has, so it is given no other",
            c.name, domain.name
        )));
    }
    Ok(())
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.19] Column `column` of base table `qn`
    /// given the declared type `written`, its names of schema `names`: a
    /// new generation of the table's storage (`made`'s, if given), its
    /// rows to be copied there, unless the type is the column's already.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such column [ISO/IEC 9075:1992
    /// §11.12 SR3], it is an identity column [§11.12 SR6], or a Syntax Rule
    /// of §11.19 is broken.
    pub(super) fn retype_column(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        (column, written): (&str, &TypeName),
        (made, action): (Option<&Made>, &AlterAction),
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        let desc = self.base_table(qn)?;
        let (at, c) =
            desc.column_desc(column).ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        super::nullability::not_identity(qn, c)?;
        retypable(&desc, at)?;
        let d = self.new_type(qn, written, names)?;
        if !widening::predefined(&c.data_type) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.19 SR14] column {column} of {qn} is of {}, no predefined type",
                c.data_type
            )));
        }
        if let Some(clause) = widening::narrowed(&c.data_type, &d) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.19 {clause}] column {column} of {qn} is of {}, which {d} may not replace",
                c.data_type
            )));
        }
        self.unchecked(&desc, column)?;
        self.unread(&desc, (at, column))?;
        if d == c.data_type {
            return Ok((Dropped::default(), Made::default(), None));
        }
        let mut next = desc.clone();
        if let Some(retyped) = next.columns.get_mut(at) {
            retyped.data_type = d;
        }
        next.generation = desc.generation.saturating_add(1);
        let reshape = Reshape::Retype(at);
        let (dropped, built) = self.regenerate(qn, (&desc, next), (reshape, Vec::new()), (made, action))?;
        Ok((dropped, built, Some(reshape)))
    }

    /// [ISO/IEC 9075-2:2023 §11.19 SR11 to SR13] The type `written`, of
    /// table `qn`'s schema's owner, its names of schema `names`: a
    /// predefined type, of no domain.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is a domain or no predefined type; as the type's
    /// resolution.
    fn new_type(&self, qn: &QualifiedName, written: &TypeName, names: &str) -> Result<DataType> {
        if let TypeName::Domain(name) = written
            && self.catalog.domain(&qualify(name, names)).is_some()
        {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.19 SR13] {} is a domain, no predefined type", name.ident)));
        }
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let d = self.data_type_of(written, (names, sole(&*self.catalog, &owner)))?;
        if !widening::predefined(&d) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.19 SR13] {d} is no predefined type")));
        }
        Ok(d)
    }

    /// [ISO/IEC 9075-2:2023 §11.19 SR6] That `column` of `desc` is
    /// referenced by the search condition of no constraint but a CHECK of
    /// its table that references it alone: no other CHECK, of its table
    /// or another, and no assertion.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one references it; as binding one.
    fn unchecked(&self, desc: &TableDesc, column: &str) -> Result<()> {
        let qn = &desc.name;
        let is_it = |u: &Use| u.0 == *qn && u.1 == column;
        for t in self.catalog.tables() {
            for k in &t.constraints {
                let ConstraintKind::Check { text, names, horizon, reads } = &k.kind else { continue };
                if t.name != *qn && !reads.contains(qn) {
                    continue;
                }
                let uses = self.check_uses(&t.name, (text, names, horizon))?;
                let alone = t.name == *qn && uses.iter().all(is_it);
                if uses.iter().any(is_it) && !alone {
                    return Err(referenced(&format!("constraint {}", k.name), "SR6", (qn, column)));
                }
            }
        }
        for a in self.catalog.assertions() {
            if self.assertion_uses(a)?.iter().any(is_it) {
                return Err(referenced(&format!("assertion {}", a.name), "SR6", (qn, column)));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.19 SR7 to SR10] That the column at `at` of
    /// `desc`, `column`, is in no trigger's column list or triggered
    /// action column set, no routine's body, no view's query and no
    /// generation expression.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is; as binding one of them.
    fn unread(&self, desc: &TableDesc, (at, column): (usize, &str)) -> Result<()> {
        let qn = &desc.name;
        let is_it = |u: &Use| u.0 == *qn && u.1 == column;
        for t in self.catalog.triggers() {
            let listed = t.table == *qn && lists(&t.event, column);
            if listed || t.columns.iter().any(is_it) {
                return Err(referenced(&format!("trigger {}", t.name), "SR7", (qn, column)));
            }
        }
        // A body the table's column is not counted of still names it if
        // its text does, as a column's RENAME reads it (RULES.md D83).
        for r in self.catalog.routines().into_iter().filter(|r| r.reads.contains(qn)) {
            if super::rename::names_any(&r.body, [column, column], false) || self.routine_usage_columns(r)?.iter().any(is_it) {
                return Err(referenced(&format!("routine {}", r.specific), "SR8", (qn, column)));
            }
        }
        for v in self.catalog.views().into_iter().filter(|v| v.reads.contains(qn)) {
            if view_uses(self, v)?.iter().any(is_it) {
                return Err(referenced(&format!("view {}", v.name), "SR9", (qn, column)));
            }
        }
        for (generated, e) in generations(self, desc)? {
            let mut used = Vec::new();
            expr_columns(&e, &mut used);
            if used.contains(&at) {
                let name = desc.columns.get(generated).map_or("", |c| c.name.as_str());
                return Err(referenced(&format!("the generation expression of column {name}"), "SR10", (qn, column)));
            }
        }
        Ok(())
    }
}

/// Whether trigger event `event` is an UPDATE whose trigger column list
/// names `column`.
fn lists(event: &TriggerEvent, column: &str) -> bool {
    let TriggerEvent::Update(columns) = event else { return false };
    columns.iter().any(|c| c == column)
}

/// [ISO/IEC 9075-2:2023 §11.19] The refusal of rule `clause`: `what`
/// references column `column` of `table`.
fn referenced(what: &str, clause: &str, (table, column): (&QualifiedName, &str)) -> crate::error::SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §11.19 {clause}] {what} references {table}.{column}, so its data type is not changed"))
}
