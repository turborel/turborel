//! \[EXTENSION\] Table constraints defined [ISO/IEC 9075:1992 §11.4, §11.6
//! to §11.9]: a table's NOT NULL, UNIQUE, PRIMARY KEY, FOREIGN KEY and CHECK
//! constraints described for the catalog, each named [§10.6]; its foreign
//! keys resolved once every table of the schema is known, so a table may
//! reference one defined after it, or itself. What they require of the rows
//! is checked at the end of each statement (`db/integrity.rs`). Split from
//! db/schema.rs (RULES.md R-SIZE-2).
//!

use super::State;
use crate::ast::{
    Check, ColumnConstraintKind, ColumnDefinition, Literal, NamedTable, QuerySpecification, References, SelectItem, SelectList,
    SelectSublist, TableConstraint, TableConstraintKind, TableElement, TableExpression, TableName, TableReference, ValueExpression,
    ValueSpecification,
};
use crate::binder::{self, Context, Purpose, Uses, bind_select, qualify};
use crate::catalog::{
    ConstraintAttributes, ConstraintDesc, ConstraintKind, ForeignKey, Horizon, QualifiedName, TableDesc, applicable, owner_of,
};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Cond, LogicalPlan};
use crate::params::Params;
use crate::parser::Parser;
use crate::value::Decimal;

/// A FOREIGN KEY of a table, described but not yet resolved: its name, the
/// positions of its referencing columns, what it references, and where it
/// is written.
pub(in crate::db) struct Pending {
    pub(super) name: QualifiedName,
    columns: Vec<usize>,
    references: References,
    at: usize,
    attributes: ConstraintAttributes,
}

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}

/// The constraints of a table being described: its name and columns, the
/// names taken in its schema, and what has been described so far.
pub(in crate::db) struct Described<'a> {
    pub(super) table: &'a QualifiedName,
    pub(super) columns: &'a [crate::catalog::ColumnDesc],
    pub(super) taken: Vec<QualifiedName>,
    pub(in crate::db) constraints: Vec<ConstraintDesc>,
    pub(in crate::db) pending: Vec<Pending>,
    /// The names written for constraints still to describe, which one the
    /// implementation names may not take [§11.6 SR2].
    reserved: Vec<QualifiedName>,
    /// The `<constraint attributes>` of the constraint being described.
    attributes: ConstraintAttributes,
}

impl<'a> Described<'a> {
    /// Nothing described yet, of table `table` with `columns`, in a schema
    /// where `taken` are the constraint names.
    pub(in crate::db) fn new(
        table: &'a QualifiedName,
        columns: &'a [crate::catalog::ColumnDesc],
        taken: Vec<QualifiedName>,
    ) -> Described<'a> {
        Described {
            table,
            columns,
            taken,
            constraints: Vec::new(),
            pending: Vec::new(),
            reserved: Vec::new(),
            attributes: ConstraintAttributes::default(),
        }
    }

    /// [§10.6 SR1, SR2; §11.6 SR2] The name of a constraint: `written`, in
    /// the table's schema and not another constraint's there, or else one
    /// of the implementation's, `TABLE_KIND1` and on, that is free.
    fn name(&mut self, written: Option<&TableName>, kind: &str) -> Result<QualifiedName> {
        let auth = &self.table.auth;
        let name = match written {
            Some(n) => {
                if let Some(other) = n.auth.as_ref().filter(|a| *a != auth) {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §10.6 SR1] constraint {other}.{} is not in schema {auth}", n.ident)));
                }
                let name = QualifiedName { auth: auth.clone(), ident: n.ident.clone() };
                if self.taken.contains(&name) {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §10.6 SR2] schema {auth} has a constraint {}", n.ident)));
                }
                name
            }
            None => (1..=self.taken.len().saturating_add(self.reserved.len()).saturating_add(1))
                .map(|k| QualifiedName { auth: auth.clone(), ident: format!("{}_{kind}{k}", self.table.ident) })
                .find(|n| !self.taken.contains(n) && !self.reserved.contains(n))
                .unwrap_or_else(|| QualifiedName { auth: auth.clone(), ident: format!("{}_{kind}", self.table.ident) }),
        };
        self.taken.push(name.clone());
        Ok(name)
    }

    /// Reserves the names `written`, of constraints still to describe.
    pub(in crate::db) fn reserve<'n>(&mut self, written: impl Iterator<Item = &'n TableName>) {
        let auth = self.table.auth.clone();
        self.reserved.extend(written.map(|n| QualifiedName { auth: auth.clone(), ident: n.ident.clone() }));
    }

    /// The positions of the columns `names` [§11.7 SR4, §11.8 SR6]: each of
    /// the table, none twice.
    fn positions(&self, names: &[String], rule_cited: &str) -> Result<Vec<usize>> {
        let mut positions = Vec::new();
        for n in names {
            let at = self
                .columns
                .iter()
                .position(|c| c.name == *n)
                .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 {rule_cited}] {} has no column {n}", self.table)))?;
            if positions.contains(&at) {
                return Err(rule(format!("[ISO/IEC 9075:1992 {rule_cited}] column {n} is named twice")));
            }
            positions.push(at);
        }
        Ok(positions)
    }

    /// Adds the constraint `kind`, named `name` if written.
    fn add(&mut self, name: Option<&TableName>, word: &str, kind: ConstraintKind) -> Result<()> {
        let name = self.name(name, word)?;
        self.constraints.push(ConstraintDesc { name, kind, attributes: self.attributes, domain: None });
        Ok(())
    }

    /// Adds the constraints of the column at `at`, `def` [§11.4].
    pub(in crate::db) fn column(&mut self, at: usize, def: &ColumnDefinition) -> Result<()> {
        for c in &def.constraints {
            self.attributes = c.attributes;
            let added = match &c.kind {
                ColumnConstraintKind::NotNull => self.add(c.name.as_ref(), "NOTNULL", ConstraintKind::NotNull(at)),
                ColumnConstraintKind::Unique { primary } => {
                    self.add(c.name.as_ref(), word(*primary), ConstraintKind::Unique { columns: vec![at], primary: *primary })
                }
                ColumnConstraintKind::References(r) => self.foreign_key(c.name.as_ref(), vec![at], r, c.at),
                ColumnConstraintKind::Check(check) => self.check(c.name.as_ref(), check),
            };
            added.or_at(c.at)?;
        }
        Ok(())
    }

    /// Adds the `<table constraint definition>` `c` [§11.6].
    pub(super) fn table_constraint(&mut self, c: &TableConstraint) -> Result<()> {
        self.attributes = c.attributes;
        let added = match &c.kind {
            TableConstraintKind::Unique { primary, columns } => {
                let columns = self.positions(columns, "§11.7 SR4")?;
                self.add(c.name.as_ref(), word(*primary), ConstraintKind::Unique { columns, primary: *primary })
            }
            TableConstraintKind::ForeignKey { columns, references } => {
                let columns = self.positions(columns, "§11.8 SR6")?;
                self.foreign_key(c.name.as_ref(), columns, references, c.at)
            }
            TableConstraintKind::Check(check) => self.check(c.name.as_ref(), check),
        };
        added.or_at(c.at)
    }

    /// A FOREIGN KEY, resolved later (`State::resolve_foreign_key`).
    fn foreign_key(&mut self, name: Option<&TableName>, columns: Vec<usize>, references: &References, at: usize) -> Result<()> {
        let name = self.name(name, "FOREIGN")?;
        self.pending.push(Pending { name, columns, references: references.clone(), at, attributes: self.attributes });
        Ok(())
    }

    /// A CHECK, its condition kept as written; bound once the table is
    /// known (`State::bind_checks`).
    fn check(&mut self, name: Option<&TableName>, check: &Check) -> Result<()> {
        self.add(name, "CHECK", ConstraintKind::Check { text: check.text.clone(), reads: Vec::new(), horizon: Horizon::default() })
    }

    /// [§11.7 SR5, SR6, SR7] `table` has at most one PRIMARY KEY, and no
    /// two unique constraints on the same columns.
    pub(in crate::db) fn check_unique(table: &TableDesc) -> Result<()> {
        let unique: Vec<(&Vec<usize>, bool)> = table
            .constraints
            .iter()
            .filter_map(|c| if let ConstraintKind::Unique { columns, primary } = &c.kind { Some((columns, *primary)) } else { None })
            .collect();
        if unique.iter().filter(|(_, primary)| *primary).count() > 1 {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.7 SR5, SR6] {} has more than one PRIMARY KEY", table.name)));
        }
        for (n, (a, _)) in unique.iter().enumerate() {
            let same = |b: &&Vec<usize>| a.len() == b.len() && a.iter().all(|c| b.contains(c));
            if unique.iter().skip(n.saturating_add(1)).any(|(b, _)| same(b)) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.7 SR7] {} has two unique constraints on the same columns", table.name)));
            }
        }
        Ok(())
    }
}

/// The name a constraint's kind gives it, when none is written.
fn word(primary: bool) -> &'static str {
    if primary { "PRIMARY" } else { "UNIQUE" }
}

/// The constraints of `elements`, the definition of table `table` with
/// `columns`, in a schema where `taken` are the constraint names: their
/// descriptors, and its foreign keys still to resolve.
pub(super) fn describe(
    table: &QualifiedName,
    columns: &[crate::catalog::ColumnDesc],
    elements: &[TableElement],
    taken: Vec<QualifiedName>,
) -> Result<(Vec<ConstraintDesc>, Vec<Pending>)> {
    let mut d = Described::new(table, columns, taken);
    d.reserve(elements.iter().flat_map(|el| match el {
        TableElement::Column(def) => def.constraints.iter().filter_map(|c| c.name.as_ref()).collect::<Vec<_>>(),
        TableElement::Constraint(c) => c.name.iter().collect(),
    }));
    let mut at = 0_usize;
    for el in elements {
        match el {
            TableElement::Column(def) => {
                d.column(at, def)?;
                at = at.saturating_add(1);
            }
            TableElement::Constraint(c) => d.table_constraint(c)?,
        }
    }
    let probe = TableDesc {
        name: table.clone(),
        columns: columns.to_vec(),
        constraints: d.constraints.clone(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    Described::check_unique(&probe)?;
    Ok((d.constraints, d.pending))
}

impl State {
    /// [ISO/IEC 9075:1992 §11.8] Resolves the FOREIGN KEY `p` of table
    /// `child`, of schema `auth`, against the tables the catalog has.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks; -104 if `auth` lacks
    /// REFERENCES on a referenced column [AR1].
    pub(in crate::db) fn resolve_foreign_key(&self, auth: &str, child: &TableDesc, p: &Pending) -> Result<ConstraintDesc> {
        let r = &p.references;
        let target = qualify(&r.table, auth);
        let parent = self.catalog.table(&target).ok_or_else(|| {
            let what = if self.catalog.view(&target).is_some() { "a view, not a base table [SR4]" } else { "not defined [SR2]" };
            rule(format!("[ISO/IEC 9075:1992 §11.8] {target} is {what}"))
        });
        let parent = parent.or_at(r.table.at)?;
        super::temporary::may_refer(child, parent, ("§11.8 SR4", "§11.8 SR5")).or_at(r.table.at)?;
        let referenced = match &r.columns {
            // [SR2a] a set of columns of a unique constraint of it.
            Some(names) => {
                let positions = Described::new(&target, &parent.columns, Vec::new()).positions(names, "§11.8 SR2a")?;
                let keyed = parent.unique().any(|u| u.len() == positions.len() && u.iter().all(|c| positions.contains(c)));
                if !keyed {
                    return Err(rule(format!(
                        "[ISO/IEC 9075:1992 §11.8 SR2a] the columns referenced are not those of a unique constraint of {target}"
                    ))
                    .or_at(p.at));
                }
                positions
            }
            // [SR2b] its PRIMARY KEY.
            None => parent
                .primary_key()
                .map(<[usize]>::to_vec)
                .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.8 SR2b] {target} has no PRIMARY KEY")))
                .or_at(p.at)?,
        };
        // [SR7] as many, each of the same type as its referencing column.
        let same = p.columns.len() == referenced.len() && child.types(&p.columns) == parent.types(&referenced);
        if !same {
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §11.8 SR7] the referencing columns of {} are not of the types of those referenced",
                p.name
            ))
            .or_at(p.at));
        }
        // [AR1] REFERENCES on each referenced column.
        let (held, _) = applicable(&*self.catalog, owner_of(&*self.catalog, auth), &target);
        if let Some(column) = referenced.iter().find(|c| !held.references.contains(c)) {
            let name = parent.columns.get(*column).map_or("?", |c| c.name.as_str());
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.8 AR1] {auth} may not reference {target}.{name}"))
                .or_at(p.at));
        }
        let fk = ForeignKey {
            columns: p.columns.clone(),
            table: target,
            referenced,
            match_type: r.match_type,
            on_update: r.on_update,
            on_delete: r.on_delete,
        };
        Ok(ConstraintDesc { name: p.name.clone(), kind: ConstraintKind::ForeignKey(fk), attributes: p.attributes, domain: None })
    }

    /// [ISO/IEC 9075:1992 §11.9] Binds the CHECK conditions of `desc`, a
    /// table of the catalog now, recording the tables each one's
    /// subqueries read, and, the first time, the columns it sees of them
    /// and of `desc` [§11.11 GR2, note].
    ///
    /// # Errors
    ///
    /// One does not bind, as `check_condition` says.
    pub(in crate::db) fn bind_checks(&self, desc: &mut TableDesc) -> Result<()> {
        let (table, width) = (desc.name.clone(), desc.columns.len());
        let this = desc.clone();
        for c in &mut desc.constraints {
            let ConstraintKind::Check { text, reads, horizon } = &mut c.kind else { continue };
            let cond =
                self.check_condition(&table, (text, horizon)).map_err(|e| SqlError { msg: format!("CHECK {}: {}", c.name, e.msg), ..e })?;
            *reads = cond.tables();
            for other in reads.iter().filter(|t| **t != table).filter_map(|t| self.catalog.table(t)) {
                super::temporary::may_refer(&this, other, ("§11.9 SR3", "§11.9 SR4"))?;
            }
            if horizon.0.is_empty() {
                let others: Vec<QualifiedName> = reads.iter().filter(|t| **t != table).cloned().collect();
                *horizon = Horizon(std::iter::once((table.clone(), width)).chain(Horizon::of(&*self.catalog, &others).0).collect());
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.9] The condition `text` of a CHECK of table
    /// `table`, bound over its columns, as its owner's, and as a
    /// constraint's (`Purpose::Constraint`), seeing the columns `horizon`
    /// says.
    ///
    /// # Errors
    ///
    /// It does not parse or bind: its rules [§11.9 SR1, SR2, SR5, SR6,
    /// AR1].
    pub(in crate::db) fn check_condition(&self, table: &QualifiedName, (text, horizon): (&str, &Horizon)) -> Result<Cond> {
        self.bind_check(table, (text, horizon), None)
    }

    /// [ISO/IEC 9075:1992 §21.3.18] The columns the CHECK `text` of table
    /// `table`, seeing the columns `horizon` says, references, once each.
    ///
    /// # Errors
    ///
    /// It does not bind, as `check_condition`.
    pub(crate) fn check_uses(&self, table: &QualifiedName, (text, horizon): (&str, &Horizon)) -> Result<Vec<(QualifiedName, String)>> {
        let uses = Uses::default();
        self.bind_check(table, (text, horizon), Some(&uses))?;
        Ok(binder::once_each(uses.into_inner()))
    }

    /// The CHECK `text` of `table`, bound as `check_condition` says, its
    /// column references noted in `uses`, if given.
    fn bind_check(&self, table: &QualifiedName, (text, horizon): (&str, &Horizon), uses: Option<&Uses>) -> Result<Cond> {
        let condition = Parser::new(text)?.check_condition()?;
        let from = TableReference::Table(NamedTable {
            table: TableName { auth: Some(table.auth.clone()), ident: table.ident.clone(), at: 0 },
            correlation: None,
            columns: None,
        });
        let table_expression = TableExpression { from: vec![from], where_clause: Some(condition), group_by: Vec::new(), having: None };
        // Its rows are selected as 1, so only the condition references a column.
        let one = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1_i128, 0_u32))));
        let select_list = SelectList::Values(vec![SelectSublist::Derived(SelectItem { value: one, alias: None, at: 0 })]);
        let spec = QuerySpecification { distinct: false, select_list, table_expression };
        let params = Params::new();
        // As its schema's owner, its names of its schema.
        let owner = owner_of(&*self.catalog, &table.auth);
        let cx = Context { purpose: Purpose::Constraint, horizon, uses, schema: &table.auth, ..Context::new(self, owner, &params) };
        let bound = bind_select(cx, &spec)?;
        if let LogicalPlan::Project { input, .. } = bound.plan
            && let LogicalPlan::Filter { cond, .. } = *input
        {
            return Ok(cond);
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] a CHECK of {table} did not bind to a filter of its rows")))
    }

    /// The names of the constraints of schema `auth`: its tables', its
    /// domains' and its assertions [ISO/IEC 9075:1992 §10.6 SR2].
    pub(in crate::db) fn constraint_names(&self, auth: &str) -> Vec<QualifiedName> {
        let tables =
            self.catalog.tables().into_iter().filter(|t| t.name.auth == auth).flat_map(|t| t.constraints.iter().map(|c| c.name.clone()));
        let domains =
            self.catalog.domains().into_iter().filter(|d| d.name.auth == auth).flat_map(|d| d.constraints.iter().map(|c| c.name.clone()));
        let assertions = self.catalog.assertions().into_iter().filter(|a| a.name.auth == auth).map(|a| a.name.clone());
        tables.chain(domains).chain(assertions).collect()
    }
}
