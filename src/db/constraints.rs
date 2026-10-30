//! \[EXTENSION\] Table constraints defined [ISO/IEC 9075:1992 §11.4, §11.6
//! to §11.9]: a table's NOT NULL, UNIQUE, PRIMARY KEY, FOREIGN KEY and CHECK
//! constraints described for the catalog, each named [§10.6]; its foreign
//! keys resolved once every table of the schema is known, so a table may
//! reference one defined after it, or itself. What they require of the rows
//! is checked at the end of each statement, or at commit if deferred
//! (`db/integrity.rs`). Split from db/schema.rs (RULES.md R-SIZE-2).

use super::State;
use crate::ast::{
    Check, ColumnConstraintKind, ColumnDefinition, Literal, NamedTable, QuerySpecification, References, SelectItem, SelectList,
    SelectSublist, TableConstraint, TableConstraintKind, TableElement, TableExpression, TableName, TableReference, ValueExpression,
    ValueSpecification,
};
use crate::binder::{self, Context, Purpose, Referencing, Uses, bind_select, qualify};
use crate::catalog::{
    ConstraintAttributes, ConstraintDesc, ConstraintKind, ForeignKey, Horizon, QualifiedName, TableDesc, applicable, owner_of,
};
use crate::error::{At, Code, Result, SqlError, SqlState, Subject};
use crate::lexer::kw;
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

/// Where a table's constraints are defined, which decides the schema of a
/// constraint's name written without one, and the schemas one may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::db) enum Within {
    /// A `<schema definition>`: its schema [§5.4 SR8a], and no other
    /// [§10.6 SR1].
    SchemaDefinition,
    /// A statement of a session, outside a `<schema definition>`: the
    /// session's default schema [§5.4 SR8b], or any schema written, which
    /// may be another than its table's: no Syntax Rule makes them one.
    Session,
    /// \[DESIGN\] A declared local temporary table [§13.11]: MODULE, its
    /// table's, and no other: a session's table puts no name in a schema
    /// other sessions share.
    Declared,
}

/// The constraints of a table being described: its name and columns, the
/// constraint names taken, and what has been described so far.
pub(in crate::db) struct Described<'a> {
    pub(super) table: &'a QualifiedName,
    pub(super) columns: &'a [crate::catalog::ColumnDesc],
    /// The names of constraints there are, of any schema.
    pub(super) taken: Vec<QualifiedName>,
    /// The schema of the names its CHECKs write without one [§5.4 SR8].
    names: String,
    /// Where it is defined.
    within: Within,
    pub(in crate::db) constraints: Vec<ConstraintDesc>,
    pub(in crate::db) pending: Vec<Pending>,
    /// The names written for constraints still to describe, which one the
    /// implementation names may not take [§11.6 SR2].
    reserved: Vec<QualifiedName>,
    /// The `<constraint attributes>` of the constraint being described.
    attributes: ConstraintAttributes,
}

impl<'a> Described<'a> {
    /// Nothing described yet, of table `table` with `columns`, its CHECKs'
    /// names of schema `names`, defined `within`, where `taken` are the
    /// names of the constraints there are.
    pub(in crate::db) fn new(
        (table, names, within): (&'a QualifiedName, &str, Within),
        columns: &'a [crate::catalog::ColumnDesc],
        taken: Vec<QualifiedName>,
    ) -> Described<'a> {
        Described {
            table,
            columns,
            taken,
            names: names.to_owned(),
            within,
            constraints: Vec::new(),
            pending: Vec::new(),
            reserved: Vec::new(),
            attributes: ConstraintAttributes::default(),
        }
    }

    /// [§5.4 SR8] The schema of a constraint's name written without one:
    /// that of the names the definition writes, a declared table's own.
    fn home(&self) -> &str {
        match self.within {
            Within::SchemaDefinition | Within::Session => &self.names,
            Within::Declared => &self.table.auth,
        }
    }

    /// The constraint name `written`, its schema the one written or else
    /// the implicit one [§5.4 SR8].
    fn placed(&self, written: &TableName) -> QualifiedName {
        QualifiedName { auth: written.auth.clone().unwrap_or_else(|| self.home().to_owned()), ident: written.ident.clone() }
    }

    /// Why the constraint name `name` may not be written where it is: of a
    /// schema other than the `<schema definition>`'s [§10.6 SR1] or a
    /// declared table's, or MODULE for a table of a schema.
    fn misplaced(&self, name: &QualifiedName) -> Option<SqlError> {
        let home = self.home();
        let why = match self.within {
            Within::SchemaDefinition => format!("[ISO/IEC 9075:1992 §10.6 SR1] constraint {name} is not in schema {home}"),
            Within::Declared => format!("[DESIGN] constraint {name} of a declared local temporary table is in MODULE, its table's schema"),
            Within::Session => format!(
                "[DESIGN] constraint {name}: MODULE names a session's declared local temporary tables, not a schema [ISO/IEC 9075:1992 §5.4]"
            ),
        };
        let misplaced = match self.within {
            Within::SchemaDefinition | Within::Declared => name.auth != home,
            Within::Session => name.auth == kw::MODULE,
        };
        misplaced.then(|| rule(why))
    }

    /// [§10.6 SR1, SR2; §11.6 SR2] The name of a constraint: `written`, in
    /// the schema it is placed in and not another constraint's there, or
    /// else one of the implementation's, `TABLE_KIND1` and on, in the
    /// table's schema, that is free.
    fn name(&mut self, written: Option<&TableName>, kind: &str) -> Result<QualifiedName> {
        let auth = &self.table.auth;
        let name = match written {
            Some(n) => {
                let name = self.placed(n);
                if let Some(e) = self.misplaced(&name) {
                    return Err(e);
                }
                if self.taken.contains(&name) {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §10.6 SR2] schema {} has a constraint {}", name.auth, name.ident)));
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
        let placed: Vec<QualifiedName> = written.map(|n| self.placed(n)).collect();
        self.reserved.extend(placed);
    }

    /// The positions of the columns `names` [§11.7 SR4, §11.8 SR6]: each of
    /// the table, none twice.
    fn positions(&self, names: &[String], rule_cited: &str) -> Result<Vec<usize>> {
        let mut positions = Vec::new();
        for n in names {
            let at = self.columns.iter().position(|c| c.name == *n).ok_or_else(|| {
                binder::no_column(self.table, format!("[ISO/IEC 9075:1992 {rule_cited}] {} has no column {n}", self.table), n)
            })?;
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
        let kind =
            ConstraintKind::Check { text: check.text.clone(), names: self.names.clone(), reads: Vec::new(), horizon: Horizon::default() };
        self.add(name, "CHECK", kind)
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
/// `columns`, its names of schema `names`, defined `within`, where `taken`
/// are the constraint names there are: their descriptors, and its foreign
/// keys still to resolve.
pub(super) fn describe(
    (table, names, within): (&QualifiedName, &str, Within),
    columns: &[crate::catalog::ColumnDesc],
    elements: &[TableElement],
    taken: Vec<QualifiedName>,
) -> Result<(Vec<ConstraintDesc>, Vec<Pending>)> {
    let mut d = Described::new((table, names, within), columns, taken);
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

/// [ISO/IEC 9075:1992 §11.8 SR2] The positions of the columns a foreign
/// key, written at `at`, references of table `target`, `parent`: those
/// `written`, of a unique constraint of it [SR2a], or else its PRIMARY KEY
/// [SR2b].
fn referenced_columns((target, parent): (&QualifiedName, &TableDesc), written: Option<&[String]>, at: usize) -> Result<Vec<usize>> {
    let Some(written) = written else {
        return parent
            .primary_key()
            .map(<[usize]>::to_vec)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.8 SR2b] {target} has no PRIMARY KEY")).or_at(at));
    };
    let positions =
        Described::new((target, &target.auth, Within::Session), &parent.columns, Vec::new()).positions(written, "§11.8 SR2a")?;
    let keyed = parent.unique().any(|u| u.len() == positions.len() && u.iter().all(|c| positions.contains(c)));
    if !keyed {
        return Err(rule(format!(
            "[ISO/IEC 9075:1992 §11.8 SR2a] the columns referenced are not those of a unique constraint of {target}"
        ))
        .or_at(at));
    }
    Ok(positions)
}

impl State {
    /// [ISO/IEC 9075:1992 §11.8 SR2, SR4] The base table `target`, which a
    /// foreign key references.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is a view, or not defined.
    fn referenced(&self, target: &QualifiedName) -> Result<&TableDesc> {
        self.catalog.table(target).ok_or_else(|| {
            if self.catalog.view(target).is_some() {
                return rule(format!("[ISO/IEC 9075:1992 §11.8 SR4] {target} is a view, not a base table"));
            }
            rule(format!("[ISO/IEC 9075:1992 §11.8 SR2] {target} is not defined"))
        })
    }

    /// [ISO/IEC 9075:1992 §11.8] Resolves the FOREIGN KEY `p` of table
    /// `child`, its names of schema `names`, against the tables the
    /// catalog has.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks; -104 if the owner of
    /// `child`'s schema lacks REFERENCES on a referenced column [AR1].
    pub(in crate::db) fn resolve_foreign_key(&self, names: &str, child: &TableDesc, p: &Pending) -> Result<ConstraintDesc> {
        let r = &p.references;
        let target = qualify(&r.table, names);
        let auth = owner_of(&*self.catalog, &child.name.auth);
        // [ISO/IEC 9075:1992 §18.1 GR3g] its errors of what it references
        // are about the table referenced.
        let about = |e: SqlError| e.or_about_table(target.parts());
        let parent = self.referenced(&target).map_err(about).or_at(r.table.at)?;
        super::temporary::may_refer(child, parent, ("§11.8 SR4", "§11.8 SR5")).or_at(r.table.at)?;
        let referenced = referenced_columns((&target, parent), r.columns.as_deref(), p.at).map_err(about)?;
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
        let (held, _) = applicable(&*self.catalog, auth, &target);
        if let Some(column) = referenced.iter().find(|c| !held.references.contains(c)) {
            let name = parent.columns.get(*column).map_or("?", |c| c.name.as_str());
            // [§18.1 GR3g] about the column, of the table referenced.
            let subject = Subject { column: Some(name.to_owned()), ..Subject::table(target.parts()) };
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.8 AR1] {auth} may not reference {target}.{name}"))
                .or_at(p.at)
                .about(subject));
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
            let ConstraintKind::Check { text, names, reads, horizon } = &mut c.kind else { continue };
            let cond = self.check_condition(&table, (text, names, horizon)).map_err(|e| SqlError {
                msg: format!("CHECK {}: {}", c.name, e.msg),
                pos: None,
                ..e
            })?;
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
    /// `table`, its names of schema `names`, bound over its columns, as its
    /// owner's, and as a constraint's (`Purpose::Constraint`), seeing the
    /// columns `horizon` says.
    ///
    /// # Errors
    ///
    /// It does not parse or bind: its rules [§11.9 SR1, SR2, SR5, SR6,
    /// AR1].
    pub(in crate::db) fn check_condition(&self, table: &QualifiedName, check: (&str, &str, &Horizon)) -> Result<Cond> {
        self.bind_check(table, check, (None, &Referencing::default()))
    }

    /// [ISO/IEC 9075:1992 §11.21, §11.9] The condition of a domain
    /// constraint, bound over `probe`, a table of one column, VALUE's, as
    /// `check_condition` binds a CHECK's: VALUE names no column [AR1].
    ///
    /// # Errors
    ///
    /// As `check_condition`'s.
    pub(in crate::db) fn value_condition(&self, probe: &QualifiedName, check: (&str, &str, &Horizon)) -> Result<Cond> {
        self.bind_check(probe, check, (None, &Referencing::of_value(probe)))
    }

    /// The columns the condition of a domain constraint, bound over
    /// `probe` as `value_condition` binds it, references, once each: VALUE
    /// not among them.
    ///
    /// # Errors
    ///
    /// As `value_condition`'s.
    pub(in crate::db) fn value_uses(&self, probe: &QualifiedName, check: (&str, &str, &Horizon)) -> Result<Vec<(QualifiedName, String)>> {
        let uses = Uses::default();
        self.bind_check(probe, check, (Some(&uses), &Referencing::of_value(probe)))?;
        Ok(binder::once_each(uses.into_inner()).into_iter().filter(|(table, _)| table != probe).collect())
    }

    /// [ISO/IEC 9075:1992 §21.3.18] The columns the CHECK `text` of table
    /// `table`, its names of schema `names`, seeing the columns `horizon`
    /// says, references, once each.
    ///
    /// # Errors
    ///
    /// It does not bind, as `check_condition`.
    pub(crate) fn check_uses(&self, table: &QualifiedName, check: (&str, &str, &Horizon)) -> Result<Vec<(QualifiedName, String)>> {
        let uses = Uses::default();
        self.bind_check(table, check, (Some(&uses), &Referencing::default()))?;
        Ok(binder::once_each(uses.into_inner()))
    }

    /// The CHECK `text` of `table`, bound as `check_condition` says, its
    /// column references noted in `uses`, if given, what its REFERENCES
    /// depend on in `referencing` [§11.9 AR1].
    fn bind_check(
        &self,
        table: &QualifiedName,
        (text, names, horizon): (&str, &str, &Horizon),
        (uses, referencing): (Option<&Uses>, &Referencing),
    ) -> Result<Cond> {
        let condition = Parser::new(text)?.of_definition().check_condition()?;
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
        // As its schema's owner.
        let owner = owner_of(&*self.catalog, &table.auth);
        let cx = Context {
            purpose: Purpose::Constraint,
            horizon,
            uses,
            schema: names,
            referencing: Some(referencing),
            ..Context::new(self, owner, &params)
        };
        // A position in the condition's own text is none of a statement's.
        let bound = bind_select(cx, &spec).map_err(|e| SqlError { pos: None, ..e })?;
        referencing.check(owner)?;
        if let LogicalPlan::Project { input, .. } = bound.plan
            && let LogicalPlan::Filter { cond, .. } = *input
        {
            return Ok(cond);
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] a CHECK of {table} did not bind to a filter of its rows")))
    }

    /// The names of the constraints there are, of every schema: tables',
    /// domains' and assertions [ISO/IEC 9075:1992 §10.6 SR2].
    pub(in crate::db) fn all_constraint_names(&self) -> Vec<QualifiedName> {
        let tables = self.catalog.tables().into_iter().flat_map(|t| t.constraints.iter().map(|c| c.name.clone()));
        let domains = self.catalog.domains().into_iter().flat_map(|d| d.constraints.iter().map(|c| c.name.clone()));
        let assertions = self.catalog.assertions().into_iter().map(|a| a.name.clone());
        tables.chain(domains).chain(assertions).collect()
    }

    /// The names of the constraints of schema `auth` [ISO/IEC 9075:1992
    /// §10.6 SR2], whatever schema their tables are in.
    pub(in crate::db) fn constraint_names(&self, auth: &str) -> Vec<QualifiedName> {
        self.all_constraint_names().into_iter().filter(|n| n.auth == auth).collect()
    }

    /// That the schema of each name of `constraints` and `pending` of
    /// table `of`, written outside a `<schema definition>`, may hold it
    /// (`constraint_schema`).
    ///
    /// # Errors
    ///
    /// As `constraint_schema`.
    pub(in crate::db) fn constraint_schemas(
        &self,
        of: &QualifiedName,
        (constraints, pending): (&[ConstraintDesc], &[Pending]),
    ) -> Result<()> {
        let names = constraints.iter().filter(|c| c.domain.is_none()).map(|c| &c.name).chain(pending.iter().map(|p| &p.name));
        names.into_iter().try_for_each(|name| self.constraint_schema(name, of))
    }

    /// That the schema of constraint `name`, of table or domain `of`, is
    /// one there is [ISO/IEC 9075:1992 §5.4 SR8b, GR18]; and \[EXTENSION\]
    /// owned by the owner of `of`'s, who defines it: a constraint is named
    /// in a schema only by its owner, which SQL-92 does not require
    /// (RULES.md D47).
    ///
    /// # Errors
    ///
    /// SQLCODE -103, SQLSTATE 3F000, if there is no such schema; -104 if
    /// another owns it.
    pub(in crate::db) fn constraint_schema(&self, name: &QualifiedName, of: &QualifiedName) -> Result<()> {
        if !self.catalog.schema_exists(&name.auth) {
            let msg = format!("[ISO/IEC 9075:1992 §5.4 SR8b, GR18] there is no schema {} for constraint {name}", name.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let owner = owner_of(&*self.catalog, &of.auth);
        if owner_of(&*self.catalog, &name.auth) != owner {
            let msg = format!(
                "[EXTENSION] constraint {name} is in schema {}, which {owner}, defining it, does not own (RULES.md D47)",
                name.auth
            );
            return Err(SqlError::new(Code::Privilege, msg));
        }
        Ok(())
    }
}
