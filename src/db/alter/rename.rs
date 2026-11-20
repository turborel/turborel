//! \[EXTENSION\] ALTER TABLE … RENAME \[COLUMN\] … TO …: a column of a base
//! table given a new name. SQL:2023 renames no column [ISO/IEC 9075-2:2023
//! §11.10], so this is an extension of the `<alter column action>`s
//! (RULES.md D83).
//!
//! The column keeps its place, its values and its descriptor but its
//! name, so its rows are not copied. What refers to it by its place, a
//! key, an index, a NOT NULL, a privilege, refers to it still. What names
//! it as a name is changed with it: the period it bounds, its period
//! constraint, written again, and each table option of columns, as
//! COLUMNSTORE.ORDER (RULES.md D31). What names it in a text read again,
//! a view's query, a CHECK, an assertion, a trigger, a routine's body, a
//! generation expression or an index's key part, would name another
//! column or none, so while one that reads the table names the column or
//! its new name, or a view selects all of the table's columns by `*`, the
//! rename is refused, as
//! DROP COLUMN RESTRICT is refused [ISO/IEC 9075:1992 §11.15 SR4].

use std::sync::Arc;

use super::storage::columns_named;
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::rule;
use crate::error::Result;
use crate::lexer::{Tok, delim, kw, lex};
use crate::settings::Kind;
use crate::value::Value;

/// A name written delimited, its quotes doubled.
pub(super) fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Whether `text` names `names` as identifiers, or, if `star`, selects all
/// columns by `*`, after SELECT or a qualifier. A text that does not lex
/// is taken to name them.
pub(super) fn names_any(text: &str, names: [&str; 2], star: bool) -> bool {
    let Ok(tokens) = lex(text) else { return true };
    tokens.iter().enumerate().any(|(k, t)| {
        if let Tok::Identifier(n) = &t.tok {
            return names.contains(&n.as_str());
        }
        let before = k.checked_sub(1).and_then(|b| tokens.get(b)).map(|b| &b.tok);
        star && t.tok == Tok::Delimiter(delim::ASTERISK) && matches!(before, Some(Tok::KeyWord(kw::SELECT) | Tok::Delimiter(delim::PERIOD)))
    })
}

/// The periods of `next` that `column` bounds bounded by `to`, and the
/// text of each one's period constraint, CHECK (start < end), written
/// again of their names.
fn renamed_bounds(next: &mut TableDesc, column: &str, to: &str) {
    for p in &mut next.periods {
        if !p.bounds(column) {
            continue;
        }
        for bound in [&mut p.start, &mut p.end] {
            if bound == column {
                to.clone_into(bound);
            }
        }
        let Some(name) = &p.constraint else { continue };
        let Some(c) = next.constraints.iter_mut().find(|c| c.name == *name) else { continue };
        if let ConstraintKind::Check { text, .. } = &mut c.kind {
            *text = format!("{} < {}", quoted(&p.start), quoted(&p.end));
        }
    }
}

impl State {
    /// \[EXTENSION\] Renames column `column` of base table `qn` `to`: its
    /// descriptor, its period's, its period constraint's text and its
    /// table's options of columns.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such column [ISO/IEC 9075:1992
    /// §11.12 SR3], has one named `to` already [§11.4 SR1] or a period of
    /// that name [ISO/IEC 9075-2:2023 §11.3 SR2d], is a typed table, whose
    /// columns are its type's attributes, or a text that reads the table
    /// names either name.
    pub(in crate::db) fn rename_column(&mut self, qn: &QualifiedName, column: &str, to: &str) -> Result<()> {
        let desc = self.base_table(qn)?;
        let at = desc.column(column).ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        if desc.column(to).is_some() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.4 SR1] {qn} already has a column {to}")));
        }
        if desc.periods.iter().any(|p| p.name == to) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR2d] {qn} has a period {to}, which no column is named")));
        }
        if desc.typed.is_some() {
            return Err(rule(format!("[EXTENSION] {qn} is a typed table, whose columns are its type's attributes, so none is renamed")));
        }
        self.unnamed_in_texts(&desc, [column, to])?;
        let mut next = desc.clone();
        if let Some(c) = next.columns.get_mut(at) {
            to.clone_into(&mut c.name);
        }
        renamed_bounds(&mut next, column, to);
        self.renamed_options(&mut next, column, to);
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(next);
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// The options of `next` that name columns with `column` named `to` in
    /// each list.
    fn renamed_options(&self, next: &mut TableDesc, column: &str, to: &str) {
        let declared = self.options_of(&next.engine);
        for (name, value) in &mut next.options {
            if !declared.iter().any(|o| o.name == *name && matches!(o.kind, Kind::Columns { .. })) {
                continue;
            }
            let names = columns_named(value);
            if names.iter().any(|n| n == column) {
                let renamed: Vec<String> = names.iter().map(|n| quoted(if n == column { to } else { n })).collect();
                *value = Value::Char(renamed.join(", "));
            }
        }
    }

    /// That no text that reads `desc`'s table names either of `names`: a
    /// view, by its query, or by `*`; a CHECK of a table, but a period
    /// constraint of it; an assertion; a trigger, on the table or reading
    /// it; a routine; a generation expression of the table; or a key part
    /// of its index.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one does.
    fn unnamed_in_texts(&self, desc: &TableDesc, names: [&str; 2]) -> Result<()> {
        let qn = &desc.name;
        let reads = |r: &[QualifiedName]| r.contains(qn);
        let periodic = |t: &TableDesc, c: &crate::catalog::ConstraintDesc| t.periods.iter().any(|p| p.constraint.as_ref() == Some(&c.name));
        let mut found: Option<String> = None;
        for v in self.catalog.views().into_iter().filter(|v| reads(&v.reads)) {
            found = found.or_else(|| names_any(&v.query, names, true).then(|| format!("view {}", v.name)));
        }
        for t in self.catalog.tables() {
            for c in &t.constraints {
                let ConstraintKind::Check { text, reads: r, .. } = &c.kind else { continue };
                let read = (t.name == *qn && !periodic(t, c)) || reads(r);
                found = found.or_else(|| (read && names_any(text, names, false)).then(|| format!("constraint {c}")));
            }
        }
        for a in self.catalog.assertions() {
            let ConstraintKind::Check { text, reads: r, .. } = &a.kind else { continue };
            found = found.or_else(|| (reads(r) && names_any(text, names, false)).then(|| format!("assertion {a}")));
        }
        for t in self.catalog.triggers().into_iter().filter(|t| t.table == *qn || reads(&t.reads)) {
            found = found.or_else(|| names_any(&t.text, names, false).then(|| format!("trigger {}", t.name)));
        }
        for r in self.catalog.routines().into_iter().filter(|r| reads(&r.reads)) {
            found = found.or_else(|| names_any(&r.body, names, false).then(|| format!("routine {}", r.specific)));
        }
        for ix in self.catalog.indexes(qn) {
            let in_key = ix.expressions.iter().any(|e| names_any(&e.text, names, false));
            found = found.or_else(|| in_key.then(|| format!("index {}", ix.name)));
        }
        for c in desc.columns.iter().filter(|c| c.generation().is_some_and(|g| names_any(&g.text, names, false))) {
            found = found.or_else(|| Some(format!("the generation expression of column {}", c.name)));
        }
        match found {
            Some(what) => Err(rule(format!(
                "[EXTENSION] {what} names {qn}.{} or {} in its text, so the column is renamed only once nothing does",
                names[0], names[1]
            ))),
            None => Ok(()),
        }
    }
}
