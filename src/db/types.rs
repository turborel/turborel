//! \[EXTENSION\] Data types beyond the standard, plug-ins (RULES.md D53):
//! registered with the database before it is opened from its files, as a
//! storage engine is, since its catalog and log may hold values of them.

use super::{Database, State, api, rule};
use crate::ast::{TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::DomainDesc;
use crate::diagnostics::Extension;
use crate::error::{At, Result};
use crate::lexer::authorization_identifier;
use crate::value::types::{ExtensionType, MAX_PARAMS, TypeParams, TypeRegistry};
use crate::value::{DataType, SqlType};

impl Database {
    /// \[EXTENSION\] Registers a data type beyond the standard, under its
    /// kind's name, which a `<data type>` writes, and its values' bytes in
    /// the catalog, the log and on the wire carry (RULES.md D53). Registered
    /// before the database is opened from its files, if they hold values of
    /// it. A type registered lives as long as the program, as its code does:
    /// every value of it refers to it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is not a regular identifier written as SQL
    /// text takes it (in upper case, no key word), or a type of its name is
    /// registered already.
    pub fn register_type(&self, kind: Box<dyn SqlType>) -> Result<()> {
        let name = kind.name().to_owned();
        if authorization_identifier(&name).ok().as_deref() != Some(name.as_str()) {
            return Err(api(format!("[DESIGN] a type is named by a regular identifier in upper case, no key word, not {name}")));
        }
        if self.snapshot()?.types.get(&name).is_some() {
            return Err(api(format!("[DESIGN] a type {name} is registered already")));
        }
        let kind: &'static dyn SqlType = Box::leak(kind);
        let done = self.change(|st| st.types.add(kind));
        self.registered((Extension::DataType, &name), done)
    }

    /// \[EXTENSION\] The data types beyond the standard registered (RULES.md
    /// D53): for reading values of them, as a server reads a client's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is poisoned.
    pub fn types(&self) -> Result<TypeRegistry> {
        Ok(self.snapshot()?.types.clone())
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.4 SR5, AR1; §6.10 SR1, AR1] The data type
    /// `of` names, in schema `schema`, for `auth`, and its domain, if of
    /// one: a domain, as `auth` holds USAGE on it; else, a name not
    /// qualified and no domain's, a type beyond the standard, registered
    /// (RULES.md D53), which takes no privilege.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names neither, or a type beyond the standard with
    /// parameters it does not take; -104 if `auth` holds no USAGE on the
    /// domain.
    pub(crate) fn resolve_type(&self, of: &TypeName, (schema, auth): (&str, &str)) -> Result<(DataType, Option<&DomainDesc>)> {
        let extension = |name: &TableName, params: &[u32]| -> Result<DataType> {
            let kind =
                self.types.kind(&name.ident).filter(|_| name.auth.is_none()).ok_or_else(|| {
                    rule(format!("[ISO/IEC 9075:1992 §11.4 SR4] there is no domain or type {}", name.ident)).or_at(name.at)
                })?;
            kind.check_params(params).or_at(name.at)?;
            let params = TypeParams::of(params)
                .ok_or_else(|| rule(format!("[DESIGN] the type {} takes at most {MAX_PARAMS} parameters", name.ident)))?;
            Ok(DataType::Extension(ExtensionType { kind, params }))
        };
        match of {
            TypeName::Data(t) => Ok((*t, None)),
            TypeName::Domain(name) => {
                let qn = qualify(name, schema);
                if name.auth.is_none() && self.catalog.domain(&qn).is_none() && self.types.get(&name.ident).is_some() {
                    return Ok((extension(name, &[])?, None));
                }
                let domain = self.usable_domain(&qn, auth).or_at(name.at)?;
                Ok((domain.data_type, Some(domain)))
            }
            TypeName::Extension { name, params } => Ok((extension(name, params)?, None)),
        }
    }
}
