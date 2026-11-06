//! \[EXTENSION\] [ISO/IEC 9075-2:2023 §4.3.6, §5.4 SR13a] Which collation a
//! name is, and what a table's columns and indexes are collated by (RULES.md
//! D60). The built-ins are of INFORMATION_SCHEMA; `<collation definition>`s
//! are in the catalog.

use std::collections::BTreeSet;

use std::sync::Arc;

use super::ddl::Dropped;
use super::definitions::Defined;
use super::revoke::Abandoned;
use super::usage::usage_alone;
use super::{Completion, Session, State};
use crate::ast::{CollationDefinition, PrivilegeDefinition, RevokeStatement, TableName, ValueSpecification};
use crate::binder::{self, qualify};
use crate::catalog::{
    Catalog, CollationDesc, ColumnDesc, ColumnDomain, ConstraintDesc, ConstraintKind, DomainDesc, IndexDesc, QualifiedName, TableDesc,
    UsageDesc, ViewDesc, owner_of,
};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::lexer::{Tok, delim, kw, lex};
use crate::params::{ParameterType, Params};
use crate::parser::Parser;
use crate::system::INFORMATION_SCHEMA;
use crate::value::collation::{CollationRef, default_pad};
use crate::value::{DataType, Value};

/// Which collation a statement of a session binds under.
#[derive(Debug, Clone)]
pub(super) enum Collating {
    /// The session's, as SET COLLATION last set it.
    AsSet,
    /// The one a prepared statement run now was prepared under.
    Fixed(Option<QualifiedName>),
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §9.15 SR4c] The session's collation, as a
    /// statement bound now takes it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2H000 if it was dropped since.
    pub(super) fn session_collation(&self, st: &State) -> Result<Option<CollationRef>> {
        let name = match &*self.prepared_collation.borrow() {
            Collating::AsSet => self.collation.borrow().clone(),
            Collating::Fixed(name) => name.clone(),
        };
        let Some(name) = name else { return Ok(None) };
        match st.collation_named(&name) {
            Some(found) => Ok(Some(found)),
            None => invalid_name(&name.to_string()),
        }
    }

    /// [ISO/IEC 9075-2:2023 §19.10] SET COLLATION `v`, or SET NO COLLATION
    /// for none. It begins no transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `v` is no character string \[SR1\]; SQLSTATE 2H000
    /// if its value, trimmed, names no collation \[GR1a\].
    pub(super) fn set_collation(&self, st: &State, v: Option<&ValueSpecification>, params: &Params) -> Result<()> {
        let Some(v) = v else {
            self.collation.replace(None);
            return Ok(());
        };
        let (value, t) = binder::value_specification_as(v, &self.auth(), params, ParameterType::Varying)?;
        let Value::Char(text) = value.clone() else {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §19.10 SR1] SET COLLATION takes a character string, not {t}"),
            ));
        };
        let trimmed = text.trim_matches(' ');
        let named = Parser::new(trimmed).and_then(|mut p| p.only_table_name()).ok().map(|name| collation_name(&name));
        match named.filter(|name| st.collation_named(name).is_some()) {
            Some(name) => {
                self.collation.replace(Some(name));
                Ok(())
            }
            None => invalid_name(trimmed),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §19.10 GR1a] Invalid collation name.
fn invalid_name<T>(name: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::INVALID_COLLATION_NAME, format!("[ISO/IEC 9075-2:2023 §19.10 GR1a] no collation {name}"))
}

impl State {
    /// The collation of `name`, if there is one.
    pub(crate) fn collation_named(&self, name: &QualifiedName) -> Option<CollationRef> {
        if name.auth == INFORMATION_SCHEMA {
            let collation = self.collations.get(&name.ident)?;
            return Some(CollationRef { collation: Arc::clone(collation), pad: default_pad(&name.ident) });
        }
        let made = self.catalog.collation(name)?;
        let collation = self.collations.get(&made.base)?;
        Some(CollationRef { collation: Arc::clone(collation), pad: made.pad })
    }

    /// The collation `c` is by, if not the default: a comparison of it by
    /// the default stays bare.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it names a collation that is not there.
    pub(crate) fn column_collation(&self, c: &ColumnDesc) -> Result<Option<CollationRef>> {
        let Some(name) = &c.collation else { return Ok(None) };
        let found = self.collation_named(name).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no collation {name}")))?;
        Ok((!found.is_default()).then_some(found))
    }

    /// `values`, of `columns` of `table`, each by its column's collation:
    /// its sort key if not the default, so equal by it is equal.
    ///
    /// # Errors
    ///
    /// As `column_collation`.
    pub(crate) fn normal(&self, table: &QualifiedName, columns: &[usize], values: &[Value]) -> Result<Vec<Value>> {
        let Some(desc) = self.catalog.table(table) else { return Ok(values.to_vec()) };
        let mut out = Vec::with_capacity(values.len());
        for (&c, v) in columns.iter().zip(values) {
            let collation = desc.columns.get(c).map_or(Ok(None), |column| self.column_collation(column))?;
            out.push(collation.map_or_else(|| v.clone(), |collation| collation.sort_key(v)));
        }
        Ok(out)
    }

    /// `ix`, on a column of `table`, with what its key is collated by.
    ///
    /// # Errors
    ///
    /// As `column_collation`.
    pub(crate) fn with_key_collations(&self, ix: IndexDesc, table: &TableDesc) -> Result<IndexDesc> {
        let collations = ix
            .columns
            .iter()
            .map(|&c| table.columns.get(c).map_or(Ok(None), |column| self.column_collation(column)))
            .collect::<Result<Vec<_>>>()?;
        let collations = if collations.iter().any(Option::is_some) { collations } else { Vec::new() };
        Ok(IndexDesc { collations, ..ix })
    }

    /// The collation a `<collate clause>` in schema `schema`'s definitions,
    /// or a session's statement, names [§5.4 SR13a, §10.7 SR1]: its name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none; -104 if `auth`, its schema's owner or
    /// the session's identifier, holds no USAGE on it [§10.7 AR1].
    pub(crate) fn collation_of(&self, name: &TableName, auth: &str) -> Result<QualifiedName> {
        let qn = collation_name(name);
        if self.collation_named(&qn).is_none() {
            return Err(SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §10.7 SR1] no collation {qn}")).or_at(name.at));
        }
        let holds = self.catalog.collation(&qn).is_none_or(|c| c.usage_of(auth, owner_of(&*self.catalog, &qn.auth)).0);
        if !holds {
            let msg = format!("[ISO/IEC 9075-2:2023 §10.7 AR1] {auth} holds no USAGE on collation {qn}");
            return Err(SqlError::new(Code::Privilege, msg).or_at(name.at));
        }
        Ok(qn)
    }

    /// [ISO/IEC 9075-2:2023 §6.1 SR3, §11.4] The declared type collation of a
    /// column or domain of schema `schema`, of type `t` and domain `of`, its
    /// `<collate clause>` `collate`: that, else its domain's.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not of a character string, or as
    /// `collation_of`.
    pub(super) fn declared_collation(
        &self,
        schema: &str,
        (t, of): (DataType, Option<&ColumnDomain>),
        collate: Option<&TableName>,
    ) -> Result<Option<QualifiedName>> {
        let Some(name) = collate else {
            return Ok(of.and_then(|d| self.catalog.domain(&d.name)).and_then(|d| d.collation.clone()));
        };
        if !t.is_character() {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §6.1 SR3] a <collate clause> is of a character string, not {t}"),
            ));
        }
        self.collation_of(name, owner_of(&*self.catalog, schema)).map(Some)
    }

    /// [ISO/IEC 9075-2:2023 §11.43] CREATE COLLATION `c` by `auth`, its name
    /// in `schema` if it names none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one of its name exists \[SR3\], or there is no
    /// collation it comes from \[SR4\]; -104 if `auth` holds no USAGE on
    /// that one \[AR2\].
    pub(super) fn create_collation(&mut self, (auth, schema): (&str, &str), c: &CollationDefinition) -> Result<Defined> {
        let name = qualify(&c.name, schema);
        if self.collation_named(&name).is_some() {
            return Err(
                SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §11.43 SR3] collation {name} exists")).or_at(c.name.at)
            );
        }
        let from = self.collation_of(&c.from, auth)?;
        let source = self.collation_named(&from).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no collation {from}")))?;
        let desc = CollationDesc { name, base: source.collation.name().to_owned(), pad: c.pad.unwrap_or(source.pad), usage: Vec::new() };
        let mut catalog = self.catalog.duplicate();
        catalog.put_collation(desc);
        self.catalog = Arc::from(catalog);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075:1992 §11.36] GRANT USAGE `p` on a collation, by `auth`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no collation definitions made, or the
    /// privilege is other than USAGE; -104 if `auth` holds none grantably.
    pub(super) fn grant_collation_usage(&mut self, auth: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let qn = collation_name(&p.table);
        let mut desc = self.made_collation(&qn)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let (held, grantable) = desc.usage_of(auth, &owner);
        if !held {
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.36 AR1] {auth} holds no USAGE on collation {qn}")));
        }
        let mut defined = Defined::default();
        if !grantable {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED);
            return Ok(defined);
        }
        for grantee in &p.grantees {
            desc.usage.retain(|u| !(u.grantor == auth && u.grantee == *grantee));
            desc.usage.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option });
        }
        self.put_collation(desc);
        Ok(defined)
    }

    /// [ISO/IEC 9075:1992 §11.37] REVOKE USAGE `r` on a collation, by `auth`.
    ///
    /// # Errors
    ///
    /// As `grant_collation_usage`; SQLCODE -103 if a schema's owner, left
    /// without it, has a column of it \[SR18\], with CASCADE too, as only
    /// DROP COLLATION copies a table's rows to revert it.
    pub(super) fn revoke_collation_usage(&mut self, (auth, _): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        usage_alone(r.actions.as_deref())?;
        let qn = collation_name(&r.table);
        let mut desc = self.made_collation(&qn)?;
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let granted = |grantee: &Option<String>| desc.usage.iter().any(|u| u.grantor == auth && u.grantee == *grantee);
        let warned = !r.grantees.iter().all(granted);
        if r.grant_option_for {
            desc.usage.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            desc.usage.retain(|u| !named(u));
        }
        // [§11.37 SR18] nothing of it left to one whose schema still uses it.
        let owner_of_it = owner_of(&*self.catalog, &qn.auth);
        let loses = |schema: &str| !desc.usage_of(owner_of(&*self.catalog, schema), owner_of_it).0;
        let of = |c: &ColumnDesc| c.collation.as_ref() == Some(&qn);
        if self.catalog.tables().into_iter().any(|t| loses(&t.name.auth) && t.columns.iter().any(of)) {
            let msg =
                format!("[ISO/IEC 9075:1992 §11.37 SR18] revoking USAGE on {qn} abandons a column of it: DROP COLLATION reverts them");
            return Err(SqlError::new(Code::SyntaxRule, msg));
        }
        self.put_collation(desc);
        let mut defined = Defined::default();
        if warned {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED);
        }
        Ok(defined)
    }

    /// [ISO/IEC 9075-2:2023 §11.44] Drops collation `qn`, whose columns are
    /// reverted already (`Session::revert_columns`): domains of it revert
    /// too \[GR2\], and with `cascade`, the views and constraints that name
    /// it go \[GR3\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such collation \[SR2\], or, not `cascade`,
    /// a view or constraint names it \[SR4\].
    pub(super) fn drop_collation(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        self.made_collation(qn)?;
        let abandoned = self.naming_collation(qn);
        if !cascade && !abandoned.is_empty() {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §11.44 SR4] a view or constraint names collation {qn}, so it is dropped only with CASCADE"),
            ));
        }
        let (dropped, _) = self.destroy(Arc::clone(&self.catalog), &abandoned)?;
        let mut catalog = self.catalog.duplicate();
        for domain in self.catalog.domains().into_iter().filter(|d| d.collation.as_ref() == Some(qn)) {
            catalog.put_domain(DomainDesc { collation: None, ..domain.clone() });
        }
        let kept: Vec<ViewDesc> =
            self.catalog.views().into_iter().filter(|v| v.columns.iter().any(|c| c.collation.as_ref() == Some(qn))).cloned().collect();
        for mut view in kept {
            view.columns.iter_mut().filter(|c| c.collation.as_ref() == Some(qn)).for_each(|c| c.collation = None);
            catalog.remove_view(&view.name);
            catalog.add_views(vec![view]);
        }
        catalog.remove_collation(qn);
        self.catalog = Arc::from(catalog);
        Ok(dropped)
    }

    /// What names collation `qn` in its text.
    fn naming_collation(&self, qn: &QualifiedName) -> Abandoned {
        let names = |c: &ConstraintDesc| matches!(&c.kind, ConstraintKind::Check { text, .. } if collates(text, qn));
        let mut abandoned = Abandoned::default();
        abandoned.views = self.catalog.views().into_iter().filter(|v| collates(&v.query, qn)).map(|v| v.name.clone()).collect();
        for t in self.catalog.tables() {
            abandoned
                .constraints
                .extend(t.constraints.iter().filter(|c| c.domain.is_none() && names(c)).map(|c| (t.name.clone(), c.clone())));
        }
        for d in self.catalog.domains() {
            abandoned.domain_constraints.extend(d.constraints.iter().filter(|c| names(c)).map(|c| (d.name.clone(), c.name.clone())));
        }
        abandoned.assertions = self.catalog.assertions().into_iter().filter(|a| names(a)).map(|a| a.name.clone()).collect();
        abandoned
    }

    /// \[DESIGN\] [ISO/IEC 9075:1992 §11.2 GR5] The collations of schema
    /// `auth`, which is being dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a column or domain of another schema is of one: its
    /// rows would need copying, which DROP COLLATION does first.
    pub(super) fn drop_schema_collations(&mut self, auth: &str) -> Result<Dropped> {
        let mine: Vec<QualifiedName> =
            self.catalog.collations().into_iter().filter(|c| c.name.auth == auth).map(|c| c.name.clone()).collect();
        let mut dropped = Dropped::default();
        for qn in mine {
            let outside = self.catalog.tables().into_iter().any(|t| t.columns.iter().any(|c| c.collation.as_ref() == Some(&qn)));
            if outside {
                let msg = format!("[DESIGN] a table outside schema {auth} is of its collation {qn}: DROP COLLATION it first");
                return Err(SqlError::new(Code::Unsupported, msg));
            }
            dropped.absorb(self.drop_collation(&qn, true)?);
        }
        Ok(dropped)
    }

    /// The descriptor of the collation `qn`, one a definition made.
    fn made_collation(&self, qn: &QualifiedName) -> Result<CollationDesc> {
        let found = self.catalog.collation(qn).cloned();
        found.ok_or_else(|| {
            SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §10.7 SR1] no collation {qn} that a definition made"))
        })
    }

    /// `desc`, recorded.
    fn put_collation(&mut self, desc: CollationDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_collation(desc);
        self.catalog = Arc::from(catalog);
    }

    /// The version of each collation there is, to keep with a database.
    pub(super) fn collation_versions(&self) -> Vec<(String, String)> {
        self.collations.iter().map(|(name, c)| (name.clone(), c.version().to_owned())).collect()
    }

    /// That each collation `catalog`'s columns and domains use is of the
    /// version `kept` says: an index holds the order it was made in, so
    /// one of another version would miss rows it has.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one is not.
    pub(super) fn check_collation_versions(&self, catalog: &dyn Catalog, kept: &[(String, String)]) -> Result<()> {
        let columns = catalog.tables().into_iter().flat_map(|t| t.columns.iter().filter_map(|c| c.collation.as_ref()));
        let domains = catalog.domains().into_iter().filter_map(|d| d.collation.as_ref());
        let used: BTreeSet<String> = columns.chain(domains).filter_map(|n| base_of(catalog, n)).collect();
        let now = self.collation_versions();
        match changed_collation(kept, &now, &used) {
            Some(name) => Err(SqlError::new(
                Code::Api,
                format!(
                    "[DESIGN] the database's indexes are in the order of another version of collation {name}, which this build does not have"
                ),
            )),
            None => Ok(()),
        }
    }
}

/// The built-in collation `name` orders by.
fn base_of(catalog: &dyn Catalog, name: &QualifiedName) -> Option<String> {
    if name.auth == INFORMATION_SCHEMA {
        return Some(name.ident.clone());
    }
    catalog.collation(name).map(|c| c.base.clone())
}

/// A collation of `used` that `kept` says was of a version other than `now`
/// has, or that `now` lacks, if any.
fn changed_collation<'u>(kept: &[(String, String)], now: &[(String, String)], used: &'u BTreeSet<String>) -> Option<&'u str> {
    let version = |list: &'_ [(String, String)], name: &str| list.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    used.iter().find(|name| version(kept, name) != version(now, name)).map(String::as_str)
}

/// Whether the search condition or query `text` has a `<collate clause>`
/// naming `qn` by its schema, as a collation a definition made is named.
fn collates(text: &str, qn: &QualifiedName) -> bool {
    let Ok(tokens) = lex(text) else { return false };
    tokens.windows(4).any(|w| {
        let [collate, schema, period, name] = w else { return false };
        collate.tok == Tok::KeyWord(kw::COLLATE)
            && schema.tok == Tok::Identifier(qn.auth.clone())
            && period.tok == Tok::Delimiter(delim::PERIOD)
            && name.tok == Tok::Identifier(qn.ident.clone())
    })
}

/// [ISO/IEC 9075-2:2023 §5.4 SR13a] The name a `<collation name>` written
/// gives: of INFORMATION_SCHEMA if it names no schema.
fn collation_name(name: &TableName) -> QualifiedName {
    QualifiedName { auth: name.auth.clone().unwrap_or_else(|| INFORMATION_SCHEMA.to_owned()), ident: name.ident.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D60: a collation a database uses, of
    /// another version or gone, is found; one it does not use, or of the
    /// same version, is not.
    #[test]
    fn a_changed_collation_is_found() {
        let pair = |n: &str, v: &str| (n.to_owned(), v.to_owned());
        let (kept, now) = ([pair("UNICODE", "1"), pair("OTHER", "1")], [pair("UNICODE", "2"), pair("OTHER", "1")]);
        let used = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<BTreeSet<String>>();
        assert_eq!(changed_collation(&kept, &now, &used(&["UNICODE"])), Some("UNICODE"));
        assert_eq!(changed_collation(&kept, &now, &used(&["OTHER"])), None);
        assert_eq!(changed_collation(&kept, &[pair("OTHER", "1")], &used(&["UNICODE"])), Some("UNICODE"), "gone");
        assert_eq!(changed_collation(&kept, &now, &used(&[])), None);
    }
}
