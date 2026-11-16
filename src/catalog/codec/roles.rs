//! \[DESIGN\] Roles and role authorizations [ISO/IEC 9075-2:2023 §4.42.3]
//! in the catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! roles:         names (count u32, each str), authorizations (count u32,
//!                each an authorization)
//! authorization: role (str), grantor (str), grantee (u8 1 and str, or
//!                u8 0 for PUBLIC), grantable u8
//! ```

use super::super::{RoleAuthorization, Roles};
use super::{count, flag, put_flag};
use crate::bytes::{Put, Reader};
use crate::error::Result;

/// `roles` in bytes: each role's name, then each authorization.
pub(crate) fn put_roles(out: &mut Vec<u8>, roles: &Roles) {
    out.put_u32(count(roles.names.len()));
    roles.names.iter().for_each(|n| out.put_str(n));
    out.put_u32(count(roles.authorizations.len()));
    for a in &roles.authorizations {
        out.put_str(&a.role);
        out.put_str(&a.grantor);
        put_flag(out, a.grantee.is_some());
        if let Some(grantee) = &a.grantee {
            out.put_str(grantee);
        }
        put_flag(out, a.grantable);
    }
}

/// The roles `put_roles` wrote.
pub(crate) fn roles(r: &mut Reader<'_>) -> Result<Roles> {
    let names = (0..r.u32()?).map(|_| r.str()).collect::<Result<_>>()?;
    let authorizations = (0..r.u32()?)
        .map(|_| {
            let (role, grantor) = (r.str()?, r.str()?);
            let grantee = if flag(r)? { Some(r.str()?) } else { None };
            Ok(RoleAuthorization { role, grantor, grantee, grantable: flag(r)? })
        })
        .collect::<Result<_>>()?;
    Ok(Roles { names, authorizations })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D25, D75: roles and their
    /// authorizations, granted to PUBLIC and to an identifier, grantable
    /// and not, read back as written.
    #[test]
    fn roles_read_back() {
        let authorization = |grantee: Option<&str>, grantable| RoleAuthorization {
            role: "R".to_owned(),
            grantor: "_SYSTEM".to_owned(),
            grantee: grantee.map(str::to_owned),
            grantable,
        };
        let written = Roles {
            names: vec!["R".to_owned(), "\u{e9}".to_owned()],
            authorizations: vec![authorization(None, false), authorization(Some("ANN"), true)],
        };
        let mut out = Vec::new();
        put_roles(&mut out, &written);
        let mut r = Reader::new(&out, "roles");
        assert_eq!(roles(&mut r).ok(), Some(written));
    }
}
