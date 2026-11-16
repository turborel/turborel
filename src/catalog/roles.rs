//! [ISO/IEC 9075-2:2023 §4.42.3, §4.42.4] Roles and their authorizations,
//! and `Who`, the top cell of an SQL-session's authorization stack
//! [§4.42.1.2], whose enabled authorization identifiers every privilege
//! check reads.
//!
//! One cell serves every check the standard draws. A statement is checked
//! with the session's cell, a user and perhaps a role: its current
//! privileges, those applicable for each enabled identifier \[§4.42.4\]. A
//! definition, an abandonment and a grantor are checked with a cell of
//! their one identifier alone (`Who::sole`): its applicable privileges,
//! which take in its roles' only if it is a role itself \[§4.42.4\], as
//! "applicable for" says.

use std::collections::BTreeSet;

use crate::ast::UserFunction;

/// [ISO/IEC 9075-2:2023 §4.42.3] A role authorization descriptor: `role`
/// granted to `grantee` by `grantor`, grantable WITH ADMIN OPTION.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleAuthorization {
    /// The role's name.
    pub role: String,
    /// The authorization identifier that granted it, "_SYSTEM" of the one
    /// a `<role definition>` creates [§12.4 GR2].
    pub grantor: String,
    /// The grantee: an authorization identifier, or none for PUBLIC.
    pub grantee: Option<String>,
    /// Whether the grantee may grant it, revoke it and drop it \[§4.42.3\].
    pub grantable: bool,
}

/// [ISO/IEC 9075-2:2023 §4.42.3] The roles of the SQL-environment, in the
/// order created, and every role authorization, in the order granted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roles {
    /// Each role descriptor's role name.
    pub names: Vec<String>,
    /// Each role authorization descriptor.
    pub authorizations: Vec<RoleAuthorization>,
}

impl Roles {
    /// Whether `name` is a role's.
    pub fn is_role(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// [ISO/IEC 9075-2:2023 §4.42.4] The roles applicable for each of
    /// `of`: each granted to PUBLIC, to one of `of`, or to a role
    /// applicable for one, found to a fixed point. Cycles are prohibited
    /// [§4.42.3, §12.5 SR1], and a set that only grows ends anyway.
    pub fn applicable(&self, of: &[&str]) -> BTreeSet<&str> {
        let mut found: BTreeSet<&str> = BTreeSet::new();
        loop {
            let before = found.len();
            for a in &self.authorizations {
                let to = a.grantee.as_deref();
                if to.is_none_or(|g| of.contains(&g) || found.contains(g)) {
                    found.insert(a.role.as_str());
                }
            }
            if found.len() == before {
                return found;
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §12.1 GR2] Whether `who` has the WITH ADMIN
    /// OPTION on `role`: some grantable authorization of it whose grantee
    /// is `who`, or a role applicable for `who`. Of an authorization
    /// granted to PUBLIC, too, as PUBLIC denotes every authorization
    /// identifier \[GR1\].
    pub fn administers(&self, who: &str, role: &str) -> bool {
        let applicable = self.applicable(&[who]);
        self.authorizations
            .iter()
            .any(|a| a.role == role && a.grantable && a.grantee.as_deref().is_none_or(|g| g == who || applicable.contains(g)))
    }
}

/// [ISO/IEC 9075-2:2023 §4.42.1.2] The top cell of an authorization stack:
/// a user identifier, a role name, or both, and the roles of the
/// SQL-environment it is read against.
#[derive(Debug, Clone, Copy)]
pub struct Who<'a> {
    /// The cell's user identifier, or, of a cell holding only a role, that
    /// role's name.
    pub auth: &'a str,
    /// The current role name, of a cell holding a user identifier too.
    pub role: Option<&'a str>,
    /// The roles and role authorizations of the SQL-environment.
    pub roles: &'a Roles,
}

impl<'a> Who<'a> {
    /// The cell holding only `auth`: a role name if `roles` has the role,
    /// else a user identifier.
    pub const fn sole(roles: &'a Roles, auth: &'a str) -> Who<'a> {
        Who { auth, role: None, roles }
    }

    /// The current user identifier [ISO/IEC 9075-2:2023 §4.42.1.2], the
    /// value of CURRENT_USER \[§6.4 GR6\]: none of a cell holding only a
    /// role.
    pub fn user(self) -> Option<&'a str> {
        (self.role.is_some() || !self.roles.is_role(self.auth)).then_some(self.auth)
    }

    /// The current role name [ISO/IEC 9075-2:2023 §4.42.1.2], the value of
    /// CURRENT_ROLE \[§6.4 GR8\].
    pub fn current_role(self) -> Option<&'a str> {
        self.role.or_else(|| self.roles.is_role(self.auth).then_some(self.auth))
    }

    /// [ISO/IEC 9075-2:2023 §4.42.4] Whether `id` is an enabled
    /// authorization identifier: the current user identifier, the current
    /// role name, or a role applicable for that role. With no role
    /// authorization at all, only the cell's own are, found without
    /// building the closure.
    pub fn includes(self, id: &str) -> bool {
        if id == self.auth || self.role == Some(id) {
            return true;
        }
        if self.roles.authorizations.is_empty() {
            return false;
        }
        let Some(role) = self.current_role() else { return false };
        self.roles.applicable(&[role]).contains(id)
    }

    /// [ISO/IEC 9075-2:2023 §4.42.4] Whether a privilege or role granted to
    /// `grantee`, none for PUBLIC, is current: PUBLIC's is, and an enabled
    /// identifier's.
    pub fn holds(self, grantee: Option<&str>) -> bool {
        grantee.is_none_or(|g| self.includes(g))
    }
}

/// [ISO/IEC 9075-2:2023 §6.4 GR6 to GR9] Whose a statement is: the top
/// cell of its authorization stack and its SQL-session user identifier,
/// what CURRENT_USER, CURRENT_ROLE, SESSION_USER and SYSTEM_USER give.
#[derive(Debug, Clone, Copy)]
pub struct Principal<'a> {
    /// The top cell, whose privileges it runs with.
    pub cell: Who<'a>,
    /// The SQL-session user identifier.
    pub session: &'a str,
}

impl<'a> Principal<'a> {
    /// The identity of a session whose cell and session user identifier are
    /// both `auth` alone: a definition's owner's, as its body is bound.
    pub const fn sole(roles: &'a Roles, auth: &'a str) -> Principal<'a> {
        Principal { cell: Who::sole(roles, auth), session: auth }
    }

    /// [ISO/IEC 9075-2:2023 §6.4 GR6 to GR9] The value of `f`, none for the
    /// null value. SYSTEM_USER's implementation-defined string (IV079) is
    /// the SQL-session user identifier: a library runs under no operating
    /// system user of its own.
    pub fn value(self, f: UserFunction) -> Option<&'a str> {
        match f {
            UserFunction::Current => self.cell.user(),
            UserFunction::Role => self.cell.current_role(),
            UserFunction::Session | UserFunction::System => Some(self.session),
        }
    }
}

/// A `Principal` owned: what a session keeps between statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    /// The top cell's identifier (`Who::auth`).
    pub auth: String,
    /// The top cell's role (`Who::role`).
    pub role: Option<String>,
    /// The SQL-session user identifier.
    pub session: String,
}

impl Authorization {
    /// Its identity, read against `roles`.
    pub fn of<'a>(&'a self, roles: &'a Roles) -> Principal<'a> {
        Principal { cell: Who { auth: &self.auth, role: self.role.as_deref(), roles }, session: &self.session }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granted(role: &str, grantee: Option<&str>) -> RoleAuthorization {
        RoleAuthorization { role: role.to_owned(), grantor: "_SYSTEM".to_owned(), grantee: grantee.map(str::to_owned), grantable: false }
    }

    fn roles(names: &[&str], authorizations: Vec<RoleAuthorization>) -> Roles {
        Roles { names: names.iter().map(|n| (*n).to_owned()).collect(), authorizations }
    }

    /// Verifies: [DESIGN] RULES.md D75, ISO/IEC 9075-2:2023 §4.42.4: the
    /// roles applicable for an identifier are those granted to it, to
    /// PUBLIC, and to a role applicable for it, through a chain.
    #[test]
    fn applicable_roles_follow_chains_and_public() {
        let r = roles(
            &["A", "B", "C", "P"],
            vec![granted("A", Some("ANN")), granted("B", Some("A")), granted("C", Some("BOB")), granted("P", None)],
        );
        let of_ann: Vec<&str> = r.applicable(&["ANN"]).into_iter().collect();
        assert_eq!(of_ann, ["A", "B", "P"]);
        assert_eq!(r.applicable(&["B"]).into_iter().collect::<Vec<_>>(), ["P"]);
    }

    /// Verifies: [DESIGN] RULES.md D75, ISO/IEC 9075-2:2023 §4.42.4: a cell
    /// of a user and a role enables the role and the roles applicable for
    /// it; a cell of a user alone enables no role granted it, and one of a
    /// role alone enables that role's roles.
    #[test]
    fn enabled_identifiers_of_each_cell() {
        let r = roles(&["A", "B"], vec![granted("A", Some("ANN")), granted("B", Some("A"))]);
        let user = Who::sole(&r, "ANN");
        assert!(user.includes("ANN") && !user.includes("A"), "a user's roles are not enabled until SET ROLE");
        let with_role = Who { auth: "ANN", role: Some("A"), roles: &r };
        assert!(with_role.includes("A") && with_role.includes("B") && !with_role.includes("BOB"));
        let role = Who::sole(&r, "A");
        assert!(role.includes("B"), "a role's applicable roles are its own");
        assert_eq!((role.user(), role.current_role()), (None, Some("A")));
        assert_eq!((with_role.user(), with_role.current_role()), (Some("ANN"), Some("A")));
        assert!(user.holds(None) && !user.holds(Some("B")));
    }

    /// Verifies: [DESIGN] RULES.md D75, ISO/IEC 9075-2:2023 §12.1 GR2: the
    /// WITH ADMIN OPTION is held through a grantable authorization to the
    /// identifier or to a role applicable for it, not through one that is
    /// not grantable.
    #[test]
    fn admin_option_through_applicable_roles() {
        let mut admin = granted("R", Some("B"));
        admin.grantable = true;
        let r = roles(&["R", "B"], vec![admin, granted("B", Some("ANN")), granted("R", Some("BOB"))]);
        assert!(r.administers("ANN", "R") && r.administers("B", "R"));
        assert!(!r.administers("BOB", "R"), "granted R without the admin option");
    }
}
