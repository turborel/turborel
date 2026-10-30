//! \[EXTENSION\] Domains [ISO/IEC 9075:1992 §11.21–§11.27]: their columns,
//! their constraints, and the statements that define, change and drop them.
//!
//! A column of a domain takes the domain's data type, and its default if
//! it has none of its own [§11.5 GR2b]; the column keeps the domain's name
//! and a copy of its default (`ColumnDomain`), changed as the domain is. A
//! domain constraint holds of every column of the domain [§11.21 GR5,
//! GR6]: so each column carries a copy of it, a CHECK of its table with
//! VALUE the column, which names the domain constraint it copies
//! (`ConstraintDesc::domain`); ALTER DOMAIN adds or drops the copies, and
//! DROP DOMAIN leaves them as the table's own constraints [§11.27 GR1c].

use std::sync::Arc;

use super::ddl::Dropped;
use super::defaults::default_of;
use super::definitions::Defined;
use super::{State, rule};
use crate::ast::{AlterDomain, AlterDomainAction, DomainDefinition, TableConstraint, TableConstraintKind, TypeName};
use crate::binder::qualify;
use crate::catalog::{ColumnDesc, ColumnDomain, ConstraintDesc, ConstraintKind, DomainDesc, Horizon, QualifiedName, TableDesc, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, delim, kw, lex};
use crate::logical::Cond;
use crate::parser::VALUE_COLUMN;
use crate::txn::LockMode;
use crate::value::DataType;

/// The text of a domain constraint `text` on column `column`: each VALUE
/// the column, a `<delimited identifier>` [ISO/IEC 9075:1992 §11.21 GR5].
fn on_column(text: &str, column: &str) -> String {
    let Ok(tokens) = lex(text) else { return text.to_owned() };
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut at) = (String::new(), 0);
    for t in tokens.iter().filter(|t| t.tok == Tok::KeyWord(kw::VALUE)) {
        out.extend(chars.get(at..t.start).unwrap_or_default());
        out.push('"');
        out.push_str(&column.replace('"', "\"\""));
        out.push('"');
        at = t.end;
    }
    out.extend(chars.get(at..).unwrap_or_default());
    out
}

/// The copy on column `column` of `table` of domain constraint `dc`: a
/// CHECK of the table, its name implementation-dependent, naming `dc`, its
/// names of the schema `dc`'s are of.
fn copy_of(dc: &ConstraintDesc, table: &QualifiedName, column: &str) -> ConstraintDesc {
    let name = QualifiedName { auth: table.auth.clone(), ident: format!("{} ON {}.{column}", dc.name.ident, table.ident) };
    let kind = match &dc.kind {
        ConstraintKind::Check { text, names, .. } => {
            ConstraintKind::Check { text: on_column(text, column), names: names.clone(), reads: Vec::new(), horizon: Horizon::default() }
        }
        other @ (ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_)) => other.clone(),
    };
    ConstraintDesc { name, kind, attributes: dc.attributes, domain: Some(dc.name.clone()) }
}

/// Whether a CAST in the search condition or query `text`, of schema
/// `schema`, is to `domain` [ISO/IEC 9075:1992 §6.10, §11.27 SR2].
pub(super) fn casts_to(text: &str, schema: &str, domain: &QualifiedName) -> bool {
    let Ok(tokens) = lex(text) else { return false };
    let toks: Vec<&Tok> = tokens.iter().map(|t| &t.tok).collect();
    (0..toks.len())
        .filter(|&i| toks.get(i) == Some(&&Tok::KeyWord(kw::CAST)))
        .any(|i| target(&toks, i).is_some_and(|at| names(&toks, at, (schema, domain))))
}

/// The place of the `<cast target>` of the CAST at `at` in `toks`: the
/// token after its outermost AS.
fn target(toks: &[&Tok], at: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (j, t) in toks.iter().enumerate().skip(at.saturating_add(1)) {
        if **t == Tok::Delimiter(delim::LEFT_PAREN) {
            depth = depth.saturating_add(1);
        } else if **t == Tok::Delimiter(delim::RIGHT_PAREN) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return None;
            }
        } else if **t == Tok::KeyWord(kw::AS) && depth == 1 {
            return Some(j.saturating_add(1));
        }
    }
    None
}

/// Whether the name at `at` in `toks`, in schema `schema`, is `domain`'s.
fn names(toks: &[&Tok], at: usize, (schema, domain): (&str, &QualifiedName)) -> bool {
    let Some(Tok::Identifier(first)) = toks.get(at) else { return false };
    if toks.get(at.saturating_add(1)) == Some(&&Tok::Delimiter(delim::PERIOD)) {
        let Some(Tok::Identifier(second)) = toks.get(at.saturating_add(2)) else { return false };
        return *first == domain.auth && *second == domain.ident;
    }
    schema == domain.auth && *first == domain.ident
}

impl State {
    /// [ISO/IEC 9075:1992 §11.21 SR4] That domain `qn` is in usage by none
    /// of `constraints`, its own: none CASTs to it, nor to a domain one of
    /// whose constraints it is in usage by, however far.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, naming the domain it would be in usage by.
    fn not_in_own_usage(&self, qn: &QualifiedName, constraints: &[ConstraintDesc]) -> Result<()> {
        let mut known: Vec<QualifiedName> = self.catalog.domains().into_iter().map(|d| d.name.clone()).collect();
        if !known.contains(qn) {
            known.push(qn.clone());
        }
        let named = |c: &ConstraintDesc| known.iter().filter(|d| casts_check(c, d)).cloned().collect::<Vec<_>>();
        let mut seen: Vec<QualifiedName> = constraints.iter().flat_map(named).collect();
        let mut at = 0_usize;
        while let Some(d) = seen.get(at).cloned() {
            at = at.saturating_add(1);
            if d == *qn {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.21 SR4, §11.25 SR2] domain {qn} would be in usage by its own constraints"
                )));
            }
            let further = self.catalog.domain(&d).map(|dd| dd.constraints.iter().flat_map(named).collect::<Vec<_>>()).unwrap_or_default();
            seen.extend(further.into_iter().filter(|n| !seen.contains(n)).collect::<Vec<_>>());
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.4 SR5, AR1] The data type of a column `of`,
    /// in schema `schema`, and its domain, if of one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no such domain is; -104 if the schema's owner holds
    /// no USAGE on it.
    pub(super) fn column_type(&self, schema: &str, of: &TypeName) -> Result<(DataType, Option<ColumnDomain>)> {
        let (t, domain) = self.resolve_type(of, (schema, owner_of(&*self.catalog, schema)))?;
        Ok((t, domain.map(|d| ColumnDomain { name: d.name.clone(), default: d.default.clone() })))
    }

    /// [ISO/IEC 9075:1992 §11.4 AR1, §6.10 AR1] The domain `qn`, if `auth`
    /// holds USAGE on it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no such domain is; -104 if `auth` holds no USAGE.
    pub(crate) fn usable_domain(&self, qn: &QualifiedName, auth: &str) -> Result<&DomainDesc> {
        let domain = self.catalog.domain(qn).ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.4 SR4] there is no domain {qn}")))?;
        if !domain.usage_of(auth, owner_of(&*self.catalog, &qn.auth)).0 {
            return Err(SqlError::new(
                Code::Privilege,
                format!("[ISO/IEC 9075:1992 §11.4 AR1, §6.10 AR1] {auth} holds no USAGE on domain {qn}"),
            ));
        }
        Ok(domain)
    }

    /// [ISO/IEC 9075:1992 §11.21 GR5, GR6] The copies, on `columns` of
    /// `table`, of the constraints of each one's domain.
    pub(super) fn domain_checks(&self, table: &QualifiedName, columns: &[ColumnDesc]) -> Vec<ConstraintDesc> {
        let of = |c: &ColumnDesc| Some((c.name.clone(), self.catalog.domain(&c.domain.as_ref()?.name)?.constraints.clone()));
        columns
            .iter()
            .filter_map(of)
            .flat_map(|(column, cs)| cs.into_iter().map(move |dc| (column.clone(), dc)))
            .map(|(column, dc)| copy_of(&dc, table, &column))
            .collect()
    }

    /// [ISO/IEC 9075:1992 §11.21] CREATE DOMAIN `d`, its names in schema
    /// `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks: no schema, a domain of its
    /// name [SR1], a constraint's name taken, or its constraints' rules.
    pub(super) fn create_domain(&mut self, (schema, zone): (&str, i16), d: &DomainDefinition) -> Result<Defined> {
        self.make_domain((schema, zone), d, true)
    }

    /// `create_domain`'s work: its constraints `bound`, or, in a `<schema
    /// definition>`, left to bind once its tables are made (`bind_domains`).
    pub(super) fn make_domain(&mut self, (schema, zone): (&str, i16), d: &DomainDefinition, bound: bool) -> Result<Defined> {
        let qn = qualify(&d.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[ISO/IEC 9075:1992 §11.21 SR1] there is no schema {}", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(d.name.at));
        }
        if self.catalog.domain(&qn).is_some() {
            return Err(
                rule(format!("[ISO/IEC 9075:1992 §11.21 SR1] schema {} has a domain {} already", qn.auth, qn.ident)).or_at(d.name.at)
            );
        }
        let default = d.default.as_ref().map(|o| default_of(d.data_type, o, zone)).transpose().or_at(d.name.at)?;
        let mut constraints = Vec::new();
        for c in &d.constraints {
            let made = self.domain_constraint((&qn, schema), d.data_type, (c, &constraints), bound)?;
            constraints.push(made);
        }
        self.not_in_own_usage(&qn, &constraints).or_at(d.name.at)?;
        let reads = constraints.iter().flat_map(|c| reads_of(c).to_vec()).collect::<Vec<_>>();
        let mut catalog = self.catalog.duplicate();
        catalog.put_domain(DomainDesc { name: qn, data_type: d.data_type, default, constraints, usage: Vec::new() });
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks: reads.into_iter().map(|t| (t, LockMode::Shared)).collect(), ..Defined::default() })
    }

    /// [ISO/IEC 9075:1992 §11.21 SR6, §11.9, §10.6] The descriptor of
    /// domain constraint `c` of domain `domain` of type `data_type`, its
    /// names of schema `names`, after those `earlier`: named as written, or
    /// `<domain>_CHECK<k>`; its condition bound over a column VALUE of the
    /// type, as a CHECK's is.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its name is taken, or the condition breaks a rule
    /// of a CHECK's; -104 for a privilege the schema's owner lacks.
    fn domain_constraint(
        &self,
        (domain, names): (&QualifiedName, &str),
        data_type: DataType,
        (c, earlier): (&TableConstraint, &[ConstraintDesc]),
        bound: bool,
    ) -> Result<ConstraintDesc> {
        let TableConstraintKind::Check(check) = &c.kind else {
            return Err(rule("[ISO/IEC 9075:1992 §11.21] a domain constraint is a CHECK".to_owned()));
        };
        let k = earlier.len().saturating_add(1);
        let name = c.name.as_ref().map_or_else(
            || QualifiedName { auth: domain.auth.clone(), ident: format!("{}_CHECK{k}", domain.ident) },
            |n| qualify(n, &domain.auth),
        );
        if name.auth != domain.auth || self.constraint_names(&domain.auth).contains(&name) || earlier.iter().any(|e| e.name == name) {
            return Err(
                rule(format!("[ISO/IEC 9075:1992 §10.6 SR2] constraint {name} is not a new one of schema {}", domain.auth)).or_at(c.at)
            );
        }
        let reads = if bound { self.bound_on_value(domain, data_type, (&check.text, names)).or_at(c.at)? } else { Vec::new() };
        Ok(ConstraintDesc {
            name,
            kind: ConstraintKind::Check { text: check.text.clone(), names: names.to_owned(), reads, horizon: Horizon::default() },
            attributes: c.attributes,
            domain: None,
        })
    }

    /// The tables a domain constraint's condition `text`, its names of
    /// schema `names`, reads, bound over a column VALUE of `data_type` of a
    /// table of the domain's schema, that is no other: as each copy of it
    /// will be bound.
    pub(super) fn bound_on_value(
        &self,
        domain: &QualifiedName,
        data_type: DataType,
        (text, names): (&str, &str),
    ) -> Result<Vec<QualifiedName>> {
        let (cond, probe) = self.on_value(domain, data_type, (text, names))?;
        Ok(cond.tables().into_iter().filter(|t| *t != probe).collect())
    }

    /// [ISO/IEC 9075:1992 §6.10 GR13] The constraints of `domain`, each
    /// bound over a row of a value alone, VALUE its one column: what a CAST
    /// to it checks [§4.10.3].
    ///
    /// # Errors
    ///
    /// A condition no longer binds.
    pub(crate) fn cast_checks(&self, domain: &DomainDesc) -> Result<Vec<(QualifiedName, Cond)>> {
        let mut out = Vec::new();
        for c in &domain.constraints {
            let ConstraintKind::Check { text, names, .. } = &c.kind else { continue };
            out.push((c.name.clone(), self.on_value(&domain.name, domain.data_type, (text, names))?.0));
        }
        Ok(out)
    }

    /// The domain constraint `text` of `domain`, its names of schema
    /// `names`, bound over the one column VALUE, of `data_type`, of a table
    /// of the domain's schema made for it, and that table's name.
    fn on_value(&self, domain: &QualifiedName, data_type: DataType, (text, names): (&str, &str)) -> Result<(Cond, QualifiedName)> {
        let column = ColumnDesc { name: VALUE_COLUMN.to_owned(), data_type, not_null: false, default: None, domain: None };
        let probe = QualifiedName { auth: domain.auth.clone(), ident: format!("{} VALUE", domain.ident) };
        let desc = TableDesc {
            name: probe.clone(),
            columns: vec![column],
            constraints: Vec::new(),
            engine: String::new(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        let mut staged = State::clone(self);
        let mut catalog = staged.catalog.duplicate();
        catalog.add_table(desc);
        staged.catalog = Arc::from(catalog);
        let cond = staged.value_condition(&probe, (&on_column(text, VALUE_COLUMN), names, &Horizon(vec![(probe.clone(), 1)])))?;
        Ok((cond, probe))
    }

    /// [ISO/IEC 9075:1992 §11.1 GR3] The constraints of the domains of
    /// schema `schema`, made by its `<schema definition>` before its tables,
    /// bound now that they are: so one may read a table the schema makes.
    ///
    /// # Errors
    ///
    /// A condition breaks a rule of a CHECK's.
    pub(super) fn bind_domains(&mut self, schema: &str) -> Result<()> {
        let mut domains: Vec<DomainDesc> = self.catalog.domains().into_iter().filter(|d| d.name.auth == schema).cloned().collect();
        for d in &mut domains {
            for c in &mut d.constraints {
                let ConstraintKind::Check { text, names, reads, .. } = &mut c.kind else { continue };
                *reads = self.bound_on_value(&d.name, d.data_type, (text, names))?;
            }
        }
        let mut catalog = self.catalog.duplicate();
        for d in domains {
            catalog.put_domain(d);
        }
        self.catalog = Arc::from(catalog);
        Ok(())
    }
}

/// The tables a domain constraint reads.
fn reads_of(c: &ConstraintDesc) -> &[QualifiedName] {
    match &c.kind {
        ConstraintKind::Check { reads, .. } => reads,
        ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) => &[],
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.22–§11.26] ALTER DOMAIN `a`, its names in
    /// schema `schema`, and each column of the domain with it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such domain [§11.22 SR1], it has no
    /// default to drop [§11.24 SR2], no constraint of the name to drop
    /// [§11.26 SR3], or a constraint added breaks a rule.
    pub(super) fn alter_domain(&mut self, (schema, zone): (&str, i16), a: &AlterDomain) -> Result<Defined> {
        let qn = qualify(&a.name, schema);
        let mut domain = self
            .catalog
            .domain(&qn)
            .cloned()
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.22 SR1] there is no domain {qn}")).or_at(a.name.at))?;
        let mut tables: Vec<TableDesc> =
            self.catalog.tables().into_iter().filter(|t| t.columns.iter().any(|c| of_domain(c, &qn))).cloned().collect();
        let mut validate = Vec::new();
        match &a.action {
            AlterDomainAction::SetDefault(o) => {
                domain.default = Some(default_of(domain.data_type, o, zone).or_at(a.name.at)?);
                columns_of(&mut tables, &qn).for_each(|c| set_domain_default(c, domain.default.clone()));
            }
            AlterDomainAction::DropDefault => {
                // [§11.24 GR2] each column with none of its own takes it.
                let Some(dropped) = domain.default.take() else {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §11.24 SR2] domain {qn} has no default")));
                };
                for c in columns_of(&mut tables, &qn) {
                    c.default.get_or_insert_with(|| dropped.clone());
                    set_domain_default(c, None);
                }
            }
            AlterDomainAction::AddConstraint(c) => validate = self.add_domain_constraint((&mut domain, schema), &mut tables, c)?,
            AlterDomainAction::DropConstraint(n) => {
                let name = qualify(n, &qn.auth);
                if !domain.constraints.iter().any(|c| c.name == name) {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §11.26 SR3] {name} is not a constraint of domain {qn}")).or_at(n.at));
                }
                domain.constraints.retain(|c| c.name != name);
                for t in &mut tables {
                    t.constraints.retain(|c| c.domain.as_ref() != Some(&name));
                }
            }
        }
        let locks = tables.iter().map(|t| (t.name.clone(), LockMode::Exclusive)).collect();
        let mut catalog = self.catalog.duplicate();
        for t in tables {
            catalog.replace_table(t);
        }
        catalog.put_domain(domain);
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks, validate, ..Defined::default() })
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.25] ALTER DOMAIN ADD `c`, its names of
    /// schema `schema`: added to `domain`, and copied onto each of its
    /// columns of `tables` [GR2]; the tables whose rows are to be checked.
    fn add_domain_constraint(
        &self,
        (domain, schema): (&mut DomainDesc, &str),
        tables: &mut [TableDesc],
        c: &TableConstraint,
    ) -> Result<Vec<QualifiedName>> {
        let qn = domain.name.clone();
        let made = self.domain_constraint((&qn, schema), domain.data_type, (c, &domain.constraints), true)?;
        self.not_in_own_usage(&qn, std::slice::from_ref(&made)).or_at(c.at)?;
        let mut validate = Vec::new();
        for t in tables {
            let copies: Vec<ConstraintDesc> =
                t.columns.iter().filter(|col| of_domain(col, &qn)).map(|col| copy_of(&made, &t.name, &col.name)).collect();
            t.constraints.extend(copies);
            self.bind_checks(t)?;
            validate.push(t.name.clone());
        }
        domain.constraints.push(made);
        Ok(validate)
    }
}

/// Whether column `c` is of domain `qn`.
fn of_domain(c: &ColumnDesc, qn: &QualifiedName) -> bool {
    c.domain.as_ref().is_some_and(|d| d.name == *qn)
}

/// The columns of `tables` of domain `qn`.
fn columns_of<'a>(tables: &'a mut [TableDesc], qn: &'a QualifiedName) -> impl Iterator<Item = &'a mut ColumnDesc> {
    tables.iter_mut().flat_map(|t| t.columns.iter_mut()).filter(move |c| of_domain(c, qn))
}

/// Column `c`'s copy of its domain's default made `default`.
fn set_domain_default(c: &mut ColumnDesc, default: Option<crate::catalog::ColumnDefault>) {
    if let Some(d) = c.domain.as_mut() {
        d.default = default;
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.27] Drops domain `qn`. Each column of it
    /// keeps its data type, and takes its default if it has none of its
    /// own [GR1a, GR1b]; each copy of a domain constraint on it becomes a
    /// constraint of its table, where the domain's owner could add it, and
    /// goes otherwise [GR1c]; with CASCADE, the views, constraints and
    /// assertions that CAST to it go, as the revoke of USAGE on it takes
    /// them [GR2; §11.37 SR13–SR15, GR5].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such domain [SR1], or, not `cascade`, a
    /// column, view, constraint or assertion uses it [SR2].
    pub(super) fn drop_domain(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let domain =
            self.catalog.domain(qn).cloned().ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.27 SR1] there is no domain {qn}")))?;
        let users = self.domain_users(qn);
        if let (Some(first), false) = (users.first(), cascade) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.27 SR2] domain {qn} is used by {first}, so it is dropped only with CASCADE")));
        }
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let mut dropped = Dropped::default();
        let mut catalog = self.catalog.duplicate();
        for t in self.catalog.tables().into_iter().filter(|t| t.columns.iter().any(|c| of_domain(c, qn)) || casts_any(t, qn)) {
            let mut next = t.clone();
            for c in next.columns.iter_mut().filter(|c| of_domain(c, qn)) {
                if c.default.is_none() {
                    c.default.clone_from(&domain.default);
                }
                c.domain = None;
            }
            // [GR1c ii] the owner of the table's schema may add a constraint.
            let kept = owner_of(&*self.catalog, &t.name.auth) == owner;
            let own: Vec<QualifiedName> = domain.constraints.iter().map(|c| c.name.clone()).collect();
            next.constraints.retain(|c| (kept || !c.domain.as_ref().is_some_and(|d| own.contains(d))) && !casts_check(c, qn));
            next.constraints.iter_mut().filter(|c| c.domain.as_ref().is_some_and(|d| own.contains(d))).for_each(|c| c.domain = None);
            dropped.names.push(t.name.clone());
            catalog.replace_table(next);
        }
        for other in self.catalog.domains().into_iter().filter(|d| d.name != *qn) {
            let mut next = other.clone();
            next.constraints.retain(|c| !casts_check(c, qn));
            catalog.put_domain(next);
        }
        for assertion in self.catalog.assertions().into_iter().filter(|a| casts_check(a, qn)) {
            catalog.remove_assertion(&assertion.name);
            dropped.names.push(assertion.name.clone());
        }
        catalog.remove_domain(qn);
        self.catalog = Arc::from(catalog);
        // [GR2] the views that CAST to it, and those that read them.
        for view in
            self.catalog.views().into_iter().filter(|v| casts_to(&v.query, &v.names, qn)).map(|v| v.name.clone()).collect::<Vec<_>>()
        {
            if self.catalog.view(&view).is_some() {
                dropped.absorb(self.drop_view(&view, true)?);
            }
        }
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// What uses domain `qn` [ISO/IEC 9075:1992 §11.27 SR2]: each column of
    /// it, and each view, constraint and assertion that CASTs to it, as
    /// named.
    fn domain_users(&self, qn: &QualifiedName) -> Vec<String> {
        let mut users = Vec::new();
        for t in self.catalog.tables() {
            users.extend(t.columns.iter().filter(|c| of_domain(c, qn)).map(|c| format!("column {}.{}", t.name, c.name)));
            users.extend(t.constraints.iter().filter(|c| casts_check(c, qn)).map(|c| format!("constraint {}", c.name)));
        }
        users.extend(self.catalog.views().into_iter().filter(|v| casts_to(&v.query, &v.names, qn)).map(|v| format!("view {}", v.name)));
        for d in self.catalog.domains().into_iter().filter(|d| d.name != *qn) {
            users.extend(d.constraints.iter().filter(|c| casts_check(c, qn)).map(|c| format!("constraint {}", c.name)));
        }
        users.extend(self.catalog.assertions().into_iter().filter(|a| casts_check(a, qn)).map(|a| format!("assertion {}", a.name)));
        users
    }
}

/// Whether the CHECK `c` CASTs to domain `qn`.
pub(super) fn casts_check(c: &ConstraintDesc, qn: &QualifiedName) -> bool {
    if let ConstraintKind::Check { text, names, .. } = &c.kind { casts_to(text, names, qn) } else { false }
}

/// Whether a constraint of table `t` CASTs to domain `qn`.
fn casts_any(t: &TableDesc, qn: &QualifiedName) -> bool {
    t.constraints.iter().any(|c| casts_check(c, qn))
}
