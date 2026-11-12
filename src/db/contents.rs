//! [ISO/IEC 9075-2:2023 §11.3 SR9b, SR10] The `<like clause>` and the
//! `<as subquery clause>` of a `<table definition>`, each effectively
//! replaced by the `<column definition>`s it stands for: of a `<like
//! clause>`, each column of the table it names, with its name, its data
//! type and NOT NULL if it is known not nullable; and with INCLUDING, its
//! default, its identity column's generator and its generation expression,
//! the table's name in it replaced. Of an `<as subquery clause>`, each
//! column of the query, as named. Constraints but NOT NULL are not copied
//! [NOTE 538], nor is a domain.
//!
//! \[DESIGN\] A definition is logged as its text and made again from it at
//! commit and by recovery (`db/definitions.rs`), so the columns are found
//! again then, from what the table named and the query read were when
//! the statement ran: the statement holds their names shared to its end,
//! and no session's declared local temporary table is read by a definition
//! the log keeps.

use std::sync::Arc;

use super::constraints::Within;
use super::{Session, State, StatementContext};
use crate::ast::{
    AsSubquery, ColumnConstraint, ColumnConstraintKind, ColumnDefinition, ColumnName, ConstraintAttributes, InsertSource, InsertStatement,
    LikeClause, SelectList, SelectSublist, TableDefinition, TableElement, TableName, TypeName, ValueExpression,
};
use crate::binder::{BoundView, Context, bind_view, qualify, table_columns};
use crate::catalog::{
    ColumnDefault, ColumnDesc, Generated, Generation, Identity, QualifiedName, SequenceDesc, TableDesc, TemporaryKind, applicable,
};
use crate::error::{At, Code, Result, SqlError};
use crate::lexer::{Tok, delim, lex};
use crate::params::Params;
use crate::parser::Parser;
use crate::txn::Txn;

/// What a column a `<like clause>` copies keeps beyond its `<column
/// definition>`: its default [SR9b iv 7], or its identity column's
/// generator [SR9b iv 5] or generation expression [SR9b iv 6].
#[derive(Default)]
pub(super) struct Kept {
    /// Its default, with INCLUDING DEFAULTS.
    default: Option<ColumnDefault>,
    /// Its generator, with INCLUDING IDENTITY; its generation expression,
    /// with INCLUDING GENERATED.
    generated: Option<Generated>,
}

impl Kept {
    /// `column`, described from its `<column definition>`, given what it
    /// keeps.
    pub(super) fn keep(&self, column: &mut ColumnDesc) {
        if self.default.is_some() {
            column.default.clone_from(&self.default);
        }
        if self.generated.is_some() {
            column.generated.clone_from(&self.generated);
        }
    }
}

/// A `<table element>` a table is described from: one written, or a
/// `<column definition>` a `<like clause>` or `<as subquery clause>` is
/// effectively replaced by, with what it keeps.
pub(super) enum Element<'t> {
    /// As written.
    Written(&'t TableElement),
    /// Copied.
    Copied(Box<(TableElement, Kept)>),
}

impl Element<'_> {
    /// The `<table element>`.
    pub(super) fn element(&self) -> &TableElement {
        match self {
            Element::Written(e) => e,
            Element::Copied(copied) => &copied.0,
        }
    }

    /// What it keeps beyond its `<column definition>`, if copied.
    pub(super) fn kept(&self) -> Option<&Kept> {
        match self {
            Element::Written(_) => None,
            Element::Copied(copied) => Some(&copied.1),
        }
    }
}

/// The table defined, the owner of its schema, its names' schema and
/// where its definition is.
pub(super) type Definer<'a> = (&'a QualifiedName, &'a str, &'a str, Within);

impl State {
    /// [ISO/IEC 9075-2:2023 §11.3 SR9b v, SR10g] The `<table element>`s of
    /// `t`, defined by `definer`, its `<like clause>`s and `<as subquery
    /// clause>` replaced, the tables `earlier` in the same `<schema
    /// definition>` among those a `<like clause>` may name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule they break; -104 for a privilege the owner
    /// lacks.
    pub(super) fn effective_elements<'t>(
        &self,
        definer: Definer<'_>,
        t: &'t TableDefinition,
        earlier: &[TableDesc],
    ) -> Result<Vec<Element<'t>>> {
        if let Some(s) = &t.subquery {
            return Ok(self.queried_columns(definer, s)?.into_iter().map(|c| Element::Copied(Box::new((c, Kept::default())))).collect());
        }
        let mut elements = Vec::new();
        for element in &t.elements {
            match element {
                TableElement::Like(like) => {
                    let copied = self.copied_columns(definer, like, earlier).or_at(like.at)?;
                    elements.extend(copied.into_iter().map(|copied| Element::Copied(Box::new(copied))));
                }
                TableElement::Column(_) | TableElement::Constraint(_) => elements.push(Element::Written(element)),
            }
        }
        Ok(elements)
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR9b iii, iv] The `<column definition>`s
    /// `like` stands for, each with what it keeps.
    fn copied_columns(&self, definer: Definer<'_>, like: &LikeClause, earlier: &[TableDesc]) -> Result<Vec<(TableElement, Kept)>> {
        let (table, _, names, _) = definer;
        let source = qualify(&like.table, names);
        let (lt, viewed) = self.like_source(definer, &source, earlier)?;
        // [SR9b ii] a viewed table's columns alone.
        if viewed && like.has_options() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.3 SR9b ii] {source} is a viewed table, so the <like clause> has no <like options>"
            )));
        }
        let mut copied = Vec::new();
        for c in &lt.columns {
            let mut kept = Kept::default();
            if like.defaults == Some(true) {
                kept.default.clone_from(&c.default);
            }
            if let (Some(true), Some(identity)) = (like.identity, c.identity()) {
                let sequence = SequenceDesc {
                    name: table.clone(),
                    epoch: 0,
                    origin: identity.sequence.start,
                    usage: Vec::new(),
                    ..identity.sequence.clone()
                };
                kept.generated = Some(Generated::Identity(Identity { always: identity.always, sequence }));
            }
            if let (Some(true), Some(g)) = (like.generated, c.generation()) {
                let text = renamed(&g.text, table)?;
                kept.generated = Some(Generated::Expression(Generation { text, names: g.names.clone() }));
            }
            copied.push((TableElement::Column(defined(c, like.at)), kept));
        }
        Ok(copied)
    }

    /// The table or view `source` a `<like clause>` names, a table `earlier`
    /// in the same `<schema definition>` among them, and whether it is a
    /// viewed table.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none, or it is a session's declared local
    /// temporary table and the definition is not; -104 if the owner of the
    /// schema holds no SELECT on it [AR2].
    fn like_source(&self, (_, owner, _, within): Definer<'_>, source: &QualifiedName, earlier: &[TableDesc]) -> Result<(TableDesc, bool)> {
        if let Some(lt) = earlier.iter().find(|d| d.name == *source) {
            return Ok((lt.clone(), false));
        }
        let lt = table_columns(self, source, owner)?;
        if within != Within::Declared && lt.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared) {
            return Err(rule(format!(
                "[DESIGN] {source} is a declared local temporary table, the session's alone: a definition the log keeps names none"
            )));
        }
        let in_catalog = self.catalog.table(source).is_some() || self.catalog.view(source).is_some();
        if in_catalog && !applicable(&*self.catalog, owner, source).0.select {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.3 AR2] {owner} has no SELECT privilege on {source}, whose columns the <like clause> copies"
            );
            return Err(SqlError::new(Code::Privilege, msg));
        }
        let viewed = self.catalog.view(source).is_some() || crate::system::view(source).is_some();
        Ok((lt, viewed))
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR10] The `<column definition>`s the
    /// `<as subquery clause>` `s` stands for: of each column of its query,
    /// as named, bound with the privileges of the owner of the schema.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule they break, or if the definition is in a
    /// `<schema definition>`; the query's.
    fn queried_columns(&self, (_, owner, names, within): Definer<'_>, s: &AsSubquery) -> Result<Vec<TableElement>> {
        if within != Within::Session {
            return Err(rule(
                "[DESIGN] a <table definition> with an <as subquery clause> is a statement of its own, not of a <schema definition>, as its query's rows are inserted in the statement's transaction".to_owned(),
            )
            .or_at(s.at));
        }
        let params = Params::new();
        // [ISO/IEC 9075-2:2023 §6.14 SR1a ii] its select list may draw
        // values of sequence generators.
        let cx = Context { schema: names, draws: true, ..Context::new(self, owner, &params) };
        let BoundView { bound, reads, .. } = bind_view(cx, &s.query).or_at(s.at)?;
        let declared =
            |t: &QualifiedName| self.catalog.table(t).is_some_and(|d| d.temporary.is_some_and(|d| d.kind == TemporaryKind::Declared));
        if let Some(t) = reads.iter().find(|t| declared(t)) {
            return Err(rule(format!(
                "[DESIGN] {t} is a declared local temporary table, the session's alone: a definition the log keeps reads none"
            ))
            .or_at(s.at));
        }
        let titles = column_names(s, bound.columns.iter().map(|c| c.name.clone()).collect())?;
        let columns = titles.into_iter().zip(&bound.columns).map(|((name, at), c)| {
            let desc = ColumnDesc {
                name,
                data_type: c.data_type,
                not_null: c.not_null,
                default: None,
                domain: None,
                collation: c.collation.clone(),
                generated: None,
            };
            TableElement::Column(defined(&desc, at))
        });
        Ok(columns.collect())
    }
}

impl State {
    /// [X3.135-1986 §6.3; ISO/IEC 9075:1992 §11.4] The columns of table
    /// `name`, its names of schema `names`, described from the `<column
    /// definition>`s of `effective`, each given what it keeps, and those
    /// definitions, in order.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if two columns are named alike, there is none, or for
    /// a rule a definition breaks.
    pub(super) fn described_columns<'e>(
        &self,
        (name, names, zone): (&QualifiedName, &str, i16),
        effective: &'e [Element<'_>],
    ) -> Result<(Vec<ColumnDesc>, Vec<&'e ColumnDefinition>)> {
        let (mut columns, mut defs): (Vec<ColumnDesc>, Vec<&ColumnDefinition>) = (Vec::new(), Vec::new());
        for e in effective {
            let TableElement::Column(c) = e.element() else { continue };
            if columns.iter().any(|x| x.name == c.name) {
                return Err(rule(format!("[X3.135-1986 §6.3 SR1] column {} is defined twice in {}", c.name, name.ident)).or_at(c.at));
            }
            let mut column = self.column_of((name, names, zone), c).or_at(c.at)?;
            if let Some(kept) = e.kept() {
                kept.keep(&mut column);
            }
            columns.push(column);
            defs.push(c);
        }
        if columns.is_empty() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.3 SR5] table {} has no column", name.ident)));
        }
        Ok((columns, defs))
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR10b, SR10d, SR10e] The name of each
/// column of the query of `s`, whose columns are named `named`, and where
/// it is written: of its `<column name list>`, if written.
///
/// # Errors
///
/// SQLCODE -103 if the list has two names alike or not one for each
/// column, or, with none written, a column is unnamed or two are named
/// alike.
fn column_names(s: &AsSubquery, named: Vec<Option<String>>) -> Result<Vec<(String, usize)>> {
    let Some(list) = &s.columns else {
        let mut titles: Vec<(String, usize)> = Vec::new();
        for (k, name) in named.into_iter().enumerate() {
            let Some(name) = name else {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.3 SR10b] column {} of the query has no name, so a <column name list> names it",
                    k.saturating_add(1)
                );
                return Err(rule(msg).or_at(s.at));
            };
            if titles.iter().any(|(t, _)| *t == name) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.3 SR10b] the query has two columns {name}, so a <column name list> names them"
                ))
                .or_at(s.at));
            }
            titles.push((name, s.at));
        }
        return Ok(titles);
    };
    if list.len() != named.len() {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §11.3 SR10e] the <column name list> names {} columns, and the query has {}",
            list.len(),
            named.len()
        );
        return Err(rule(msg).or_at(s.at));
    }
    let mut titles: Vec<(String, usize)> = Vec::new();
    for ColumnName { name, at } in list {
        if titles.iter().any(|(t, _)| t == name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR10d] the <column name list> names {name} twice")).or_at(*at));
        }
        titles.push((name.clone(), *at));
    }
    Ok(titles)
}

/// [ISO/IEC 9075-2:2023 §11.3 SR9b iii 5, SR10f iv] The `<column
/// definition>` of column `c`, written at `at`: its name, its data type,
/// with its collation, and NOT NULL if it is known not nullable.
fn defined(c: &ColumnDesc, at: usize) -> ColumnDefinition {
    let not_null = ColumnConstraint { name: None, at, kind: ColumnConstraintKind::NotNull, attributes: ConstraintAttributes::default() };
    ColumnDefinition {
        name: c.name.clone(),
        at,
        data_type: Some(TypeName::Data(c.data_type)),
        default: None,
        generated: None,
        constraints: if c.not_null { vec![not_null] } else { Vec::new() },
        collate: c.collation.as_ref().map(|q| TableName { auth: Some(q.auth.clone()), ident: q.ident.clone(), at }),
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR9b iv 6A] The generation expression
/// `text`, the `<table name>` of each `<column reference>` in it replaced
/// by `table`'s, every one being of the table it was defined in [§11.4
/// SR10c].
///
/// # Errors
///
/// SQLCODE -910 if it no longer reads as it did.
fn renamed(text: &str, table: &QualifiedName) -> Result<String> {
    let value = Parser::new(text)?.of_definition().generation_expression()?;
    let mut qualified = Vec::new();
    qualified_columns(&value, &mut qualified);
    qualified.sort_unstable();
    let tokens = lex(text)?;
    let mut chars: Vec<char> = text.chars().collect();
    let name = format!("\"{}\".\"{}\".", table.auth.replace('"', "\"\""), table.ident.replace('"', "\"\""));
    // From the last, so the places before stay where they were.
    for at in qualified.into_iter().rev() {
        let Some(first) = tokens.iter().position(|t| t.start == at) else { continue };
        // The column's name is the last of the <identifier>s the periods join.
        let mut last = first;
        while matches!(tokens.get(last.saturating_add(1)).map(|t| &t.tok), Some(Tok::Delimiter(d)) if *d == delim::PERIOD)
            && matches!(tokens.get(last.saturating_add(2)).map(|t| &t.tok), Some(Tok::Identifier(_)))
        {
            last = last.saturating_add(2);
        }
        let (Some(start), Some(column)) = (tokens.get(first).map(|t| t.start), tokens.get(last).map(|t| t.start)) else { continue };
        if column > start && column <= chars.len() {
            chars.splice(start..column, name.chars());
        }
    }
    Ok(chars.into_iter().collect())
}

/// Adds to `out` where each `<column reference>` in `value` written with
/// a qualifier begins, in the order they are written.
fn qualified_columns(value: &ValueExpression, out: &mut Vec<usize>) {
    if let ValueExpression::Column(c) = value
        && c.qualifier.is_some()
    {
        out.push(c.at);
    }
    value.for_each_child(&mut |child| qualified_columns(child, out));
}

/// [ISO/IEC 9075-2:2023 §11.3] The tables and views the `<like clause>`s
/// and `<as subquery clause>` of `t` name, and the sequence generators its
/// query draws values of [§6.14 SR1a ii], its names of schema `names`.
pub(super) fn contents_read(t: &TableDefinition, names: &str) -> Vec<QualifiedName> {
    let mut written: Vec<&TableName> =
        t.elements.iter().filter_map(|e| if let TableElement::Like(like) = e { Some(&like.table) } else { None }).collect();
    if let Some(s) = &t.subquery {
        s.query.table_names(&mut written);
        if let Some(SelectList::Values(items)) = s.query.spec().map(|spec| &spec.select_list) {
            for item in items {
                if let SelectSublist::Derived(d) = item {
                    drawn(&d.value, &mut written);
                }
            }
        }
    }
    written.into_iter().map(|w| qualify(w, names)).collect()
}

/// Adds to `out` the sequence generator each NEXT VALUE FOR in `value`
/// names.
fn drawn<'v>(value: &'v ValueExpression, out: &mut Vec<&'v TableName>) {
    if let ValueExpression::NextValue { name, .. } = value {
        out.push(name);
    }
    value.for_each_child(&mut |child| drawn(child, out));
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §11.3 GR10] Of a table `t` defined WITH DATA,
    /// on the definitions `next`, which have it: INSERT INTO TN QE, the
    /// query's rows inserted in transaction `tx`, how many.
    ///
    /// # Errors
    ///
    /// The `<insert statement>`'s.
    pub(super) fn inserted_data(&self, next: &Arc<State>, tx: &Txn, t: &TableDefinition) -> Result<Option<usize>> {
        let Some(s) = t.subquery.as_ref().filter(|s| s.with_data) else { return Ok(None) };
        let qn = qualify(&t.name, &self.default_schema());
        let table = TableName { auth: Some(qn.auth), ident: qn.ident, at: t.name.at };
        let insert = InsertStatement { table, columns: None, overriding: None, source: InsertSource::Query(Box::new(s.query.clone())) };
        // Each temporary table the session's own instance, the one created
        // among them (`Session::with_temporaries`).
        let st = self.with_temporaries(Arc::clone(next));
        self.insert_statement(StatementContext { st: &st, tx }, &insert, &Params::new(), None).map(Some).or_at(s.at)
    }
}

/// A Syntax Rule broken (SQLCODE -103).
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}
