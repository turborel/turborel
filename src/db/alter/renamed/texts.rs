//! \[EXTENSION\] What refuses a table's RENAME TO (RULES.md D402): a kind
//! of table whose name more than its descriptor keeps, and a text read
//! again that names it, as a text that names a column refuses RENAME
//! COLUMN (RULES.md D83). And the text of a trigger on the table, its ON
//! clause written again of the new name.

use super::super::rename::{names_any, quoted};
use crate::ast::{SchemaStatement, SqlStatement, TriggerDefinition};
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName, TableDesc, TriggerDesc};
use crate::db::State;
use crate::db::rule;
use crate::error::Result;
use crate::lexer::{Tok, delim, lex};
use crate::parser::Parser;

/// The `<trigger definition>` `text`, if it parses as one.
fn trigger_definition(text: &str) -> Option<TriggerDefinition> {
    let parsed = Parser::for_stored(text).and_then(|mut p| p.sql_statement());
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Trigger(t), .. }) = parsed else { return None };
    Some(t)
}

/// The character offsets of the `<table name>` that begins at `at` in
/// `text`: an identifier, and each `.` and identifier after it.
fn name_span(text: &str, at: usize) -> Option<(usize, usize)> {
    let tokens = lex(text).ok()?;
    let first = tokens.iter().position(|t| t.start == at)?;
    let mut end = tokens.get(first)?.end;
    let mut k = first;
    while let (Some(dot), Some(part)) = (tokens.get(k.saturating_add(1)), tokens.get(k.saturating_add(2))) {
        if dot.tok != Tok::Delimiter(delim::PERIOD) || !matches!(part.tok, Tok::Identifier(_)) {
            break;
        }
        end = part.end;
        k = k.saturating_add(2);
    }
    Some((at, end))
}

/// The text of a `<trigger definition>`, `text`, its subject table named
/// `to` in its ON clause; as it is, if it is no trigger definition.
fn renamed_text(text: &str, to: &QualifiedName) -> String {
    let span = trigger_definition(text).and_then(|t| name_span(text, t.table.at));
    let Some((start, end)) = span else { return text.to_owned() };
    let before: String = text.chars().take(start).collect();
    let after: String = text.chars().skip(end).collect();
    format!("{before}{}.{}{after}", quoted(&to.auth), quoted(&to.ident))
}

/// Trigger `t`, on table `from`, on it named `to`: its descriptor's
/// subject table, the columns of it its action names, its horizon and
/// its text's ON clause.
pub(super) fn renamed_trigger(t: &TriggerDesc, (from, to): (&QualifiedName, &QualifiedName)) -> TriggerDesc {
    let named = |n: &QualifiedName| if n == from { to.clone() } else { n.clone() };
    let widths = t.horizon.widths.iter().map(|(n, w)| (named(n), *w)).collect();
    TriggerDesc {
        table: to.clone(),
        text: renamed_text(&t.text, to),
        horizon: Horizon { widths, ..t.horizon.clone() },
        columns: t.columns.iter().map(|(n, c)| (named(n), c.clone())).collect(),
        ..t.clone()
    }
}

/// Whether `c` is a CHECK that reads `table` in a subquery, or, of
/// `table`'s own, one whose text names it as `named` says.
fn check_names(c: &ConstraintDesc, (table, own): (&QualifiedName, bool), named: &dyn Fn(&str) -> bool) -> bool {
    let ConstraintKind::Check { text, reads, .. } = &c.kind else { return false };
    reads.contains(table) || (own && named(text))
}

impl State {
    /// \[EXTENSION\] That table `desc` keeps no more of its name than its
    /// descriptor does: it is not typed, temporary, system-versioned or a
    /// node or edge table, and no vector pipeline or spatial column
    /// registration names it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is one of those.
    pub(super) fn renamable(&self, desc: &TableDesc) -> Result<()> {
        let qn = &desc.name;
        let spatial = self.catalog.spatial();
        let piped = self.catalog.embeddings().pipelines.iter().any(|p| p.source == *qn || p.target == *qn);
        let registered = spatial.columns().iter().any(|c| c.table == *qn) || spatial.lr_columns().iter().any(|c| c.table == *qn);
        let why = [
            (desc.typed.is_some(), "a typed table, which its subtable family knows by its name"),
            (desc.temporary.is_some(), "a temporary table, whose sessions' rows are kept by its name"),
            (self.versioned(qn), "a system-versioned table, whose history is kept by its name"),
            (desc.native().is_some(), "a node or edge table (§4.2), which its graph knows by its name"),
            (piped, "a table a vector pipeline keeps"),
            (registered, "a table of a column registered with a spatial or linear referencing system"),
        ];
        match why.iter().find(|(is, _)| *is) {
            Some((_, what)) => Err(rule(format!("[EXTENSION] {qn} is {what}, so it is not renamed"))),
            None => Ok(()),
        }
    }

    /// \[EXTENSION\] That no text read again names table `desc`: no view's
    /// query, CHECK of a table or a domain, assertion, trigger, routine's
    /// body or property graph that reads it; and no CHECK, generation
    /// expression or key part of its own, nor its triggers' actions, that
    /// name it, qualifying a column.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one does.
    pub(super) fn unnamed_table(&self, desc: &TableDesc) -> Result<()> {
        let qn = &desc.name;
        let ident = qn.ident.as_str();
        let named = |text: &str| names_any(text, [ident, ident], false);
        let mut found: Option<String> = None;
        for v in self.catalog.views().into_iter().filter(|v| v.reads.contains(qn)) {
            found = found.or_else(|| Some(format!("view {}", v.name)));
        }
        for t in self.catalog.tables() {
            for c in t.constraints.iter().filter(|c| check_names(c, (qn, t.name == *qn), &named)) {
                found = found.or_else(|| Some(format!("constraint {c}")));
            }
        }
        let domains = self.catalog.domains().into_iter().flat_map(|d| d.constraints.iter());
        for c in domains.chain(self.catalog.assertions()).filter(|c| check_names(c, (qn, false), &named)) {
            found = found.or_else(|| Some(format!("constraint {c}")));
        }
        for t in self.catalog.triggers() {
            let action = (t.table == *qn).then(|| trigger_definition(&t.text)).flatten();
            let acts = action.is_some_and(|d| named(&d.action_text) || d.when_text.as_deref().is_some_and(named));
            if t.reads.contains(qn) || acts {
                found = found.or_else(|| Some(format!("trigger {}", t.name)));
            }
        }
        for r in self.catalog.routines().into_iter().filter(|r| r.reads.contains(qn)) {
            found = found.or_else(|| Some(format!("routine {}", r.specific)));
        }
        for g in self.catalog.graphs().into_iter().filter(|g| g.reads.contains(qn)) {
            found = found.or_else(|| Some(format!("property graph {}", g.name)));
        }
        found = found.or_else(|| self.named_by_own(desc, &named));
        match found {
            Some(what) => Err(rule(format!("[EXTENSION] {what} names {qn} in its text, so the table is renamed only once nothing does"))),
            None => Ok(()),
        }
    }

    /// What of table `desc`'s own, a generation expression or a key part
    /// of an index, names it in its text as `named` says, if any.
    fn named_by_own(&self, desc: &TableDesc, named: &dyn Fn(&str) -> bool) -> Option<String> {
        if let Some(c) = desc.columns.iter().find(|c| c.generation().is_some_and(|g| named(&g.text))) {
            return Some(format!("the generation expression of column {}", c.name));
        }
        let ix = self.catalog.indexes(&desc.name).into_iter().find(|ix| ix.expressions.iter().any(|e| named(&e.text)))?;
        Some(format!("index {}", ix.name))
    }
}
