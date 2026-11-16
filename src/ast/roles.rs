//! Abstract syntax of the statements of roles [ISO/IEC 9075-2:2023 §12.4
//! to §12.8, §19.3].

use super::{DropBehavior, ValueSpecification};

/// [ISO/IEC 9075-2:2023 §12.8] `<grantor>` ::= CURRENT_USER |
/// CURRENT_ROLE: whose a grant is, or who administers a role created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grantor {
    /// CURRENT_USER: the current user identifier.
    CurrentUser,
    /// CURRENT_ROLE: the current role name.
    CurrentRole,
}

/// A statement of roles, an SQL-schema statement [ISO/IEC 9075-2:2023
/// §4.41.2].
#[derive(Debug)]
pub enum RoleStatement {
    /// [§12.4] `<role definition>` ::= CREATE ROLE `<role name>` \[WITH
    /// ADMIN `<grantor>`\]
    Create {
        /// The `<role name>`, and where it begins.
        name: (String, usize),
        /// WITH ADMIN's `<grantor>`, if written.
        admin: Option<Grantor>,
    },
    /// [§12.5] `<grant role statement>` ::= GRANT `<role granted>` \[{,
    /// `<role granted>`}...\] TO `<grantee>` \[{, `<grantee>`}...\] \[WITH
    /// ADMIN OPTION\] \[GRANTED BY `<grantor>`\]
    Grant {
        /// Each `<role granted>`, and where it begins.
        roles: Vec<(String, usize)>,
        /// The `<grantee>`s: an `<authorization identifier>`, or none for
        /// PUBLIC.
        grantees: Vec<Option<String>>,
        /// WITH ADMIN OPTION was written.
        admin: bool,
        /// GRANTED BY's `<grantor>`, if written.
        grantor: Option<Grantor>,
    },
    /// [§12.7] `<revoke role statement>` ::= REVOKE \[ADMIN OPTION FOR\]
    /// `<role revoked>` \[{, `<role revoked>`}...\] FROM `<grantee>` \[{,
    /// `<grantee>`}...\] \[GRANTED BY `<grantor>`\] `<drop behavior>`
    Revoke {
        /// ADMIN OPTION FOR was written: the admin option goes, the role
        /// stays granted.
        admin_option: bool,
        /// Each `<role revoked>`, and where it begins.
        roles: Vec<(String, usize)>,
        /// The `<grantee>`s, none for PUBLIC.
        grantees: Vec<Option<String>>,
        /// GRANTED BY's `<grantor>`, if written.
        grantor: Option<Grantor>,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// [§12.6] `<drop role statement>` ::= DROP ROLE `<role name>` `<drop
    /// behavior>`
    Drop {
        /// The `<role name>`, and where it begins.
        name: (String, usize),
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
}

impl RoleStatement {
    /// Where its first role name begins.
    pub fn at(&self) -> usize {
        match self {
            RoleStatement::Create { name, .. } | RoleStatement::Drop { name, .. } => name.1,
            RoleStatement::Grant { roles, .. } | RoleStatement::Revoke { roles, .. } => roles.first().map_or(0, |r| r.1),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §19.3] `<set role statement>` ::= SET ROLE `<role
/// specification>`, `<role specification>` ::= `<value specification>` |
/// NONE.
#[derive(Debug)]
pub enum SetRole {
    /// The `<value specification>`, whose value names the role.
    Role(ValueSpecification),
    /// NONE: no current role.
    None,
}
