//! [ISO/IEC 9075-2:2023 §4.42.3, §12.4 to §12.8, §19.3] Roles: what a
//! session's statements run as, its principal; SET ROLE; and the
//! statements of roles, SQL-schema statements made in a transaction as
//! GRANT is (`db/definitions.rs`), each taking one lock, of every role
//! authorization, exclusive (`roles_lock`).
//!
//! A rule read of the users, which change outside any transaction, is
//! checked as the statement runs (`Session::role_access`): who may create a
//! role (IW141), and that a role's name is no user's. One read of the
//! catalog and of the top cell, which the log keeps with the statement's
//! text, is checked as it is made (`State::role_statement`), and again as
//! it is replayed.

mod revoke;
mod settle;

use std::sync::Arc;

use super::definitions::Defined;
use super::{Session, State, rule};
use crate::ast::{DropBehavior, Grantor, RoleStatement, SetRole, ValueSpecification};
use crate::binder;
use crate::catalog::{Catalog, QualifiedName, RoleAuthorization, Roles, SYSTEM_GRANTOR, Who};
use crate::error::{At, Code, Result, SqlError, SqlState, fail, fail_with};
use crate::params::{ParameterType, Params};
use crate::system::DEFINITION_SCHEMA;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §6.2 SR] PUBLIC, which names no authorization
/// identifier a role may take.
const PUBLIC: &str = "PUBLIC";

/// The name whose lock a statement of roles takes exclusive, and a GRANT
/// of privileges shared: no object's, so no other statement takes it, as
/// the descriptors of role authorizations [ISO/IEC 9075-2:2023 §4.42.3]
/// would be named in the Definition Schema.
pub(super) fn roles_lock() -> QualifiedName {
    QualifiedName { auth: DEFINITION_SCHEMA.to_owned(), ident: "ROLE_AUTHORIZATION_DESCRIPTORS".to_owned() }
}

/// What gives a `<value specification>`'s value and type, as `value_in`
/// does for a session.
pub(super) type Valued<'v> = dyn Fn(&ValueSpecification) -> Result<(Value, DataType)> + 'v;

/// [ISO/IEC 9075-2:2023 §12.8] The grantor `g`, written or OMITTED, of a
/// statement of the cell `cell`: of OMITTED, its user identifier, else its
/// role name.
///
/// # Errors
///
/// SQLCODE -103 if CURRENT_USER is written and the cell holds no user
/// identifier, or CURRENT_ROLE and it holds no role \[SR2b, SR2c\].
pub(super) fn grantor_of(cell: Who<'_>, g: Option<Grantor>) -> Result<String> {
    let found = match g {
        None => cell.user().or_else(|| cell.current_role()),
        Some(Grantor::CurrentUser) => cell.user(),
        Some(Grantor::CurrentRole) => cell.current_role(),
    };
    found.map(str::to_owned).ok_or_else(|| {
        let which = if g == Some(Grantor::CurrentUser) { "a current user identifier" } else { "a current role name" };
        SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §12.8 SR2] there is no {which} to be the grantor"))
    })
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §6.4] The value of `v` and its type, its user
    /// value functions this session's, read against the roles of `st`: of
    /// a `?`, of the type `taken`, if given [ISO/IEC 9075:1992 §17.6 GR5].
    ///
    /// # Errors
    ///
    /// As `binder::value_specification`.
    pub(super) fn value_in(
        &self,
        st: &State,
        v: &ValueSpecification,
        params: &Params,
        taken: Option<ParameterType>,
    ) -> Result<(Value, DataType)> {
        let authorization = self.authorization();
        let user = authorization.of(st.catalog.roles());
        match taken {
            Some(t) => binder::value_specification_as(v, user, params, t),
            None => binder::value_specification(v, user, params),
        }
    }

    /// `value_in` the database's definitions as they are now.
    ///
    /// # Errors
    ///
    /// As `value_in`; SQLCODE -910 if the definitions cannot be read.
    pub(super) fn value_now(&self, v: &ValueSpecification, params: &Params, taken: Option<ParameterType>) -> Result<(Value, DataType)> {
        let st = self.db.snapshot()?;
        self.value_in(&st, v, params, taken)
    }

    /// The Access Rules of statement of roles `r` read of the users, in
    /// `st`, and the name of the lock it takes (`roles_lock`).
    ///
    /// # Errors
    ///
    /// For CREATE ROLE: SQLCODE -104 if the session's user may not create a
    /// role [§12.4 AR1, IW141]; -103 if its name is a user's [SR1].
    pub(super) fn role_access(&self, st: &State, r: &RoleStatement) -> Result<QualifiedName> {
        let RoleStatement::Create { name: (name, at), .. } = r else { return Ok(roles_lock()) };
        let authorization = self.authorization();
        let cell = authorization.of(st.catalog.roles()).cell;
        if !cell.user().is_some_and(|u| st.users.creates_roles(u)) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.4 AR1] {} may not create a role, which the database's owner and each user it allows by ALTER USER ... CREATE ROLE may",
                cell.auth
            );
            return Err(SqlError::new(Code::Privilege, msg).or_at(*at));
        }
        if st.users.exists(name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.4 SR1] {name} is a user's authorization identifier")).or_at(*at));
        }
        Ok(roles_lock())
    }

    /// [ISO/IEC 9075-2:2023 §19.3] SET ROLE `role`: the current role of the
    /// top cell of the session's authorization stack the role `role`'s
    /// value names, trimmed, or none.
    ///
    /// # Errors
    ///
    /// SQLSTATE 25001 in a transaction \[GR1\]; SQLCODE -103 if the value is
    /// not a character string \[SR1\]; SQLSTATE 0P000 if the cell holds no
    /// user identifier \[GR2\], or no authorization of the role is granted
    /// to it or to PUBLIC \[GR3, GR4\].
    pub(super) fn set_role(&self, role: &SetRole, params: &Params) -> Result<()> {
        if self.transaction().is_some() {
            return fail_with(
                Code::TransactionState,
                SqlState::ACTIVE_TRANSACTION,
                "[ISO/IEC 9075-2:2023 §19.3 GR1] SET ROLE in a transaction".to_owned(),
            );
        }
        let SetRole::Role(v) = role else {
            self.role.replace(None);
            return Ok(());
        };
        let st = self.db.snapshot()?;
        let (value, data_type) = self.value_in(&st, v, params, Some(ParameterType::Varying))?;
        if !data_type.is_character() {
            return fail(Code::SyntaxRule, "[ISO/IEC 9075-2:2023 §19.3 SR1] SET ROLE takes a character string".to_owned());
        }
        let invalid = |why: String| fail_with(Code::Privilege, SqlState::INVALID_ROLE_SPECIFICATION, why);
        let authorization = self.authorization();
        let Some(user) = authorization.of(st.catalog.roles()).cell.user() else {
            return invalid("[ISO/IEC 9075-2:2023 §19.3 GR2] there is no current user identifier to set a role for".to_owned());
        };
        let named = if let Value::Char(s) = &value { crate::lexer::authorization_identifier(s.trim_matches(' ')).ok() } else { None };
        let roles = st.catalog.roles();
        let granted = |name: &str| roles.authorizations.iter().any(|a| a.role == name && a.grantee.as_deref().is_none_or(|g| g == user));
        match named {
            Some(name) if roles.is_role(&name) && granted(&name) => {
                self.role.replace(Some(name));
                Ok(())
            }
            Some(name) => invalid(format!("[ISO/IEC 9075-2:2023 §19.3 GR3, GR4] no role {name} is granted to {user} or to PUBLIC")),
            None => invalid(format!("[ISO/IEC 9075-2:2023 §19.3 GR3] {} names no role", value.to_literal())),
        }
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §12.8] The grantor `g`, written or OMITTED, of
    /// a statement of this session, its cell read against the roles of
    /// `st`.
    ///
    /// # Errors
    ///
    /// As `grantor_of`.
    pub(super) fn grantor(&self, st: &State, g: Option<Grantor>) -> Result<String> {
        let authorization = self.authorization();
        grantor_of(authorization.of(st.catalog.roles()).cell, g)
    }

    /// The generated columns the REVOKE of roles or DROP ROLE `r` loses with
    /// CASCADE, dropped before it is made (`dropping_generations`).
    ///
    /// # Errors
    ///
    /// As `State::role_losses`.
    pub(super) fn role_generations(&self, r: &RoleStatement) -> Result<Vec<super::generated::Column>> {
        if !matches!(
            r,
            RoleStatement::Revoke { behavior: DropBehavior::Cascade, .. } | RoleStatement::Drop { behavior: DropBehavior::Cascade, .. }
        ) {
            return Ok(Vec::new());
        }
        let fresh = self.definitions(self.db.snapshot()?)?;
        let role = self.role.borrow().clone();
        let losses = fresh.role_losses((&self.auth(), role.as_deref()), r).or_at(r.at())?;
        Ok(losses.map(|l| l.lost.abandoned.columns).unwrap_or_default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR12ad] With CASCADE, the columns the
    /// REVOKE of roles or DROP ROLE `statement` impacts reverted, in
    /// transaction `tx`, as REVOKE USAGE on a collation's are
    /// (`revert_impacted`).
    ///
    /// # Errors
    ///
    /// As `revert_columns`; the statement's own, once they are reverted,
    /// the transaction then only rolling back.
    pub(super) fn revert_role_impacted(&self, tx: &crate::txn::Txn, statement: &crate::ast::SchemaStatement) -> Result<()> {
        let crate::ast::SchemaStatement::Role(r) = statement else { return Ok(()) };
        if !matches!(
            r,
            RoleStatement::Revoke { behavior: DropBehavior::Cascade, .. } | RoleStatement::Drop { behavior: DropBehavior::Cascade, .. }
        ) {
            return Ok(());
        }
        let fresh = self.definitions(self.db.snapshot()?)?;
        let role = self.role.borrow().clone();
        let Some(losses) = fresh.role_losses((&self.auth(), role.as_deref()), r).or_at(r.at())? else { return Ok(()) };
        let mut reverted = false;
        for (qn, schemas) in &losses.lost.collations {
            let tables: Vec<QualifiedName> =
                fresh.tables_of_collation(qn, None).into_iter().filter(|t| schemas.contains(&t.auth)).collect();
            if !tables.is_empty() {
                self.revert_columns(tx, qn, &tables).or_at(r.at())?;
                reverted = true;
            }
        }
        if reverted {
            let after = self.definitions(self.db.snapshot()?)?;
            if State::clone(&after).role_statement((&self.auth(), role.as_deref()), r).is_err() {
                tx.doom("a statement of roles failed with a collation's columns reverted");
            }
        }
        Ok(())
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §12.8] The grantor `g` of a statement of the
    /// cell `(auth, role)`.
    ///
    /// # Errors
    ///
    /// As `grantor_of`.
    pub(super) fn cell_grantor(&self, (auth, role): (&str, Option<&str>), g: Option<Grantor>) -> Result<String> {
        grantor_of(Who { auth, role, roles: self.catalog.roles() }, g)
    }

    /// The statement of roles `r` of the cell `(auth, role)`.
    ///
    /// # Errors
    ///
    /// Those of `create_role`, `grant_role`, `revoke_role` and `drop_role`.
    pub(super) fn role_statement(&mut self, (auth, role): (&str, Option<&str>), r: &RoleStatement) -> Result<Defined> {
        let at = r.at();
        let roles = self.catalog.roles().clone();
        let cell = Who { auth, role, roles: &roles };
        match r {
            RoleStatement::Create { name: (name, _), admin } => self.create_role(cell, name, *admin),
            RoleStatement::Grant { roles: granted, grantees, admin, grantor } => {
                self.grant_role(grantor_of(cell, *grantor)?.as_str(), granted, (grantees, *admin))
            }
            RoleStatement::Revoke { admin_option, roles: revoked, grantees, grantor, behavior } => {
                let a = grantor_of(cell, *grantor)?;
                self.revoke_role(&a, (revoked, grantees), (*admin_option, *behavior))
            }
            RoleStatement::Drop { name: (name, _), behavior } => self.drop_role(cell, name, *behavior),
        }
        .or_at(at)
    }

    /// [ISO/IEC 9075-2:2023 §12.4] CREATE ROLE `name` by the cell `cell`,
    /// WITH ADMIN `admin` if written.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `name` is an authorization identifier the database
    /// knows \[SR1\], or the grantor is not there \[SR3\].
    fn create_role(&mut self, cell: Who<'_>, name: &str, admin: Option<Grantor>) -> Result<Defined> {
        if self.known_identifier(name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.4 SR1] {name} is an authorization identifier already")));
        }
        let a = grantor_of(cell, admin)?;
        let mut roles = self.catalog.roles().clone();
        roles.names.push(name.to_owned());
        // [GR2] grantable, of the special grantor.
        let grantee = Some(a);
        roles.authorizations.push(RoleAuthorization {
            role: name.to_owned(),
            grantor: SYSTEM_GRANTOR.to_owned(),
            grantee,
            grantable: true,
        });
        self.put_roles(roles);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.5] GRANT each of `granted` TO each of
    /// `grantees` by `a`, WITH ADMIN OPTION if `admin`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a role named is not one, or a grantee is a role
    /// applicable for it, a cycle \[SR1\]; SQLSTATE 0P000 if `a` holds a
    /// role named WITH ADMIN OPTION by none of its applicable roles \[GR1\].
    fn grant_role(&mut self, a: &str, granted: &[(String, usize)], (grantees, admin): (&[Option<String>], bool)) -> Result<Defined> {
        let mut roles = self.catalog.roles().clone();
        for (role, at) in granted {
            refuse_unknown(&roles, role).or_at(*at)?;
            for grantee in grantees.iter().flatten() {
                if grantee == role || roles.applicable(&[role]).contains(grantee.as_str()) {
                    let msg = format!("[ISO/IEC 9075-2:2023 §12.5 SR1] {grantee} is {role} or a role applicable for it, a cycle");
                    return Err(rule(msg).or_at(*at));
                }
            }
            if !roles.administers(a, role) {
                let msg = format!("[ISO/IEC 9075-2:2023 §12.5 GR1] {a} holds {role} WITH ADMIN OPTION by none of its applicable roles");
                return Err(SqlError::new(Code::Privilege, msg).with_state(SqlState::INVALID_ROLE_SPECIFICATION).or_at(*at));
            }
        }
        for (role, _) in granted {
            for grantee in grantees {
                merge(
                    &mut roles,
                    RoleAuthorization { role: role.clone(), grantor: a.to_owned(), grantee: grantee.clone(), grantable: admin },
                );
            }
        }
        self.put_roles(roles);
        Ok(Defined::default())
    }

    /// The `<role definition>`s and `<grant role statement>`s of a `<schema
    /// definition>` [ISO/IEC 9075-2:2023 §11.1], each by its owner `owner`.
    ///
    /// # Errors
    ///
    /// As `role_statement`; SQLCODE -103 for another statement of roles,
    /// which no schema element is.
    pub(super) fn schema_roles(&mut self, owner: &str, roles: &[RoleStatement]) -> Result<()> {
        for r in roles {
            if let RoleStatement::Revoke { .. } | RoleStatement::Drop { .. } = r {
                return Err(rule("[ISO/IEC 9075-2:2023 §11.1] a <schema element> creates or grants a role only".to_owned()).or_at(r.at()));
            }
            self.role_statement((owner, None), r)?;
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §12.4 SR1] Whether `name` is an authorization
    /// identifier of the SQL-environment: a user's or a role's, the owner
    /// of a schema, or a grantor or grantee of a privilege or a role.
    fn known_identifier(&self, name: &str) -> bool {
        let cat = &*self.catalog;
        [PUBLIC, SYSTEM_GRANTOR].contains(&name)
            || self.users.exists(name)
            || cat.roles().is_role(name)
            || cat.schemas().into_iter().any(|s| cat.schema_owner(s) == Some(name))
            || self.any_grant(&|grantor, grantee| grantor == name || grantee == Some(name))
    }

    /// Whether some privilege of any kind, or role authorization, has a
    /// grantor and grantee, none for PUBLIC, that `of` takes.
    pub(super) fn any_grant(&self, of: &dyn Fn(&str, Option<&str>) -> bool) -> bool {
        let cat = &*self.catalog;
        let usage = |list: &[crate::catalog::UsageDesc]| list.iter().any(|u| of(&u.grantor, u.grantee.as_deref()));
        cat.roles().authorizations.iter().any(|a| of(&a.grantor, a.grantee.as_deref()))
            || granted_on_tables(cat, of)
            || cat.domains().into_iter().any(|d| usage(&d.usage))
            || cat.collations().into_iter().any(|c| usage(&c.usage))
            || cat.sequences().into_iter().any(|s| usage(&s.usage))
            || cat.user_types().into_iter().any(|t| usage(&t.usage) || usage(&t.to_distinct.execute) || usage(&t.to_source.execute))
            || cat.routines().into_iter().any(|r| usage(&r.execute) || usage(&r.debug))
            || cat.embeddings().models.iter().any(|m| usage(&m.execute))
    }

    /// `roles`, recorded.
    pub(super) fn put_roles(&mut self, roles: Roles) {
        let mut catalog = self.catalog.duplicate();
        catalog.set_roles(roles);
        self.catalog = Arc::from(catalog);
    }
}

/// Whether a privilege on a table or view of `cat` has a grantor and
/// grantee `of` takes.
fn granted_on_tables(cat: &dyn Catalog, of: &dyn Fn(&str, Option<&str>) -> bool) -> bool {
    let tables = cat.tables().into_iter().map(|t| &t.name).chain(cat.views().into_iter().map(|v| &v.name));
    tables.into_iter().any(|t| cat.privileges(t).iter().any(|p| of(&p.grantor, p.grantee.as_deref())))
}

/// That `role` is a role of `roles`.
///
/// # Errors
///
/// SQLCODE -103 if it is not.
pub(super) fn refuse_unknown(roles: &Roles, role: &str) -> Result<()> {
    if roles.is_role(role) {
        return Ok(());
    }
    Err(rule(format!("[ISO/IEC 9075-2:2023 §5.4 GR29] {role} identifies no role")))
}

/// [ISO/IEC 9075-2:2023 §12.5 GR2 to GR5] `granted` added to `roles`: one
/// the same but for being grantable made grantable, a duplicate not added.
fn merge(roles: &mut Roles, granted: RoleAuthorization) {
    let same = |a: &RoleAuthorization| a.role == granted.role && a.grantor == granted.grantor && a.grantee == granted.grantee;
    match roles.authorizations.iter_mut().find(|a| same(a)) {
        Some(a) => a.grantable |= granted.grantable,
        None => roles.authorizations.push(granted),
    }
}
