//! \[EXTENSION\] Data types beyond the standard, plug-ins (RULES.md D53):
//! registered with the database before it is opened from its files, as a
//! storage engine is, since its catalog and log may hold values of them.
//! And the data type a type name written names: a domain's, a user-defined
//! type [ISO/IEC 9075-2:2023 §6.1] (RULES.md D69), or a plug-in's.

use super::{Database, State, api, rule};
use crate::ast::{TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::{DomainDesc, owner_of};
use crate::diagnostics::Extension;
use crate::error::{At, Code, Result, fail};
use crate::lexer::authorization_identifier;
use crate::value::types::composite::nesting;
use crate::value::types::{ExtensionType, Field, MAX_PARAMS, MAX_TYPE_NESTING, TypeParams, TypeRegistry};
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
    /// registered already; or a setting it declares is refused
    /// (`Configurable`, RULES.md D36).
    pub fn register_type(&self, kind: Box<dyn SqlType>) -> Result<()> {
        let name = kind.name().to_owned();
        if authorization_identifier(&name).ok().as_deref() != Some(name.as_str()) {
            return Err(api(format!("[DESIGN] a type is named by a regular identifier in upper case, no key word, not {name}")));
        }
        if self.snapshot()?.types.get(&name).is_some() {
            return Err(api(format!("[DESIGN] a type {name} is registered already")));
        }
        let kind: &'static dyn SqlType = Box::leak(kind);
        let done = self.change(|st| {
            st.declare_plugin(&name, kind.configurable())?;
            st.types.add(kind)
        });
        self.registered((Extension::DataType, &name), done.and_then(|()| self.tell_plugin(kind.configurable())))
    }

    /// \[EXTENSION\] The data types beyond the standard registered (RULES.md
    /// D53): for reading values of them, as a server reads a client's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed, or a lock is poisoned.
    pub fn types(&self) -> Result<TypeRegistry> {
        Ok(self.snapshot()?.types.clone())
    }
}

/// [ISO/IEC 9075:1992 §11.4 SR4] The message of a name no type's.
const MISSING: &str = "[ISO/IEC 9075:1992 §11.4 SR4] there is no domain or type";

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
    ///
    /// [ISO/IEC 9075-2:2023 §6.1 AR1] A name no domain's names a
    /// user-defined type, as `auth` holds USAGE on it, before a plug-in's;
    /// and so does each type an array or row type written names.
    pub(crate) fn resolve_type(&self, of: &TypeName, (schema, auth): (&str, &str)) -> Result<(DataType, Option<&DomainDesc>)> {
        if let TypeName::Domain(name) = of {
            let qn = qualify(name, schema);
            if self.catalog.domain(&qn).is_some() {
                let domain = self.usable_domain(&qn, auth).or_at(name.at)?;
                return Ok((domain.data_type.clone(), Some(domain)));
            }
        }
        Ok((self.data_type_of(of, (schema, auth))?, None))
    }

    /// [ISO/IEC 9075-2:2023 §6.1] The `<data type>` `of`, in schema
    /// `schema`, for `auth`: each name in it a user-defined type `auth`
    /// holds USAGE on, else a plug-in's type, never a domain; nested no
    /// deeper than `MAX_TYPE_NESTING`, a distinct type's source counted.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a name is a domain's or no type's; -104 if `auth`
    /// holds no USAGE on a user-defined type [AR1]; -900 if it nests too
    /// deep.
    pub(super) fn data_type_of(&self, of: &TypeName, (schema, auth): (&str, &str)) -> Result<DataType> {
        let t = match of {
            TypeName::Data(t) => t.clone(),
            TypeName::Domain(name) => {
                if self.catalog.domain(&qualify(name, schema)).is_some() {
                    let msg = format!(
                        "[ISO/IEC 9075-2:2023 §6.1] an array's element type or a row's field type is no domain, as {} is",
                        name.ident
                    );
                    return Err(rule(msg).or_at(name.at));
                }
                self.named_type(name, (schema, auth))?
            }
            TypeName::Extension { name, params } => self.extension_type((name, params), MISSING)?,
            TypeName::Array(element, max) => DataType::array(self.data_type_of(element, (schema, auth))?, *max),
            TypeName::Row(fields) => {
                let field = |(name, t): &(String, TypeName)| -> Result<Field> {
                    Ok(Field { name: name.clone(), data_type: self.data_type_of(t, (schema, auth))?, unnamed: false })
                };
                DataType::row(fields.iter().map(field).collect::<Result<Vec<_>>>()?)
            }
        };
        if nesting(&t) > MAX_TYPE_NESTING {
            return fail(
                Code::Unsupported,
                format!("[DESIGN] the type nests arrays and rows deeper than this implementation's limit of {MAX_TYPE_NESTING}"),
            );
        }
        Ok(t)
    }

    /// [ISO/IEC 9075-2:2023 §6.1, §5.4] The type `name` names, no domain's,
    /// in schema `schema` if it names none (the SQL-path, RULES.md D69):
    /// a user-defined type, if `auth` holds USAGE on it [AR1]; else, not
    /// qualified, a type beyond the standard (RULES.md D53).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names neither; -104 if `auth` holds no USAGE.
    pub(crate) fn named_type(&self, name: &TableName, (schema, auth): (&str, &str)) -> Result<DataType> {
        let qn = qualify(name, schema);
        let Some(user_type) = self.catalog.user_type(&qn) else { return self.extension_type((name, &[]), MISSING) };
        if !user_type.usage_of(auth, owner_of(&*self.catalog, &qn.auth)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §6.1 AR1] {auth} holds no USAGE on type {qn}");
            return fail(Code::Privilege, msg).or_at(name.at);
        }
        Ok(user_type.data_type())
    }

    /// [ISO/IEC 9075:1992 §11.21] The data type of a `<domain definition>`
    /// in schema `schema`: its `<data type>`, or \[EXTENSION\] a type
    /// beyond the standard, registered (RULES.md D53). A name alone is the
    /// type, never a domain: §11.21 takes no domain of a domain.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names a domain, a user-defined type [ISO/IEC
    /// 9075-2:2023 §11.34], or no type registered, or one with parameters
    /// it does not take; or names any type as an element or a field.
    pub(crate) fn domain_type(&self, of: &TypeName, schema: &str) -> Result<DataType> {
        let missing = "[ISO/IEC 9075:1992 §11.21] there is no type";
        match of {
            TypeName::Data(t) => Ok(t.clone()),
            TypeName::Domain(name) => {
                let qn = qualify(name, schema);
                if self.catalog.domain(&qn).is_some() {
                    let msg = format!("[ISO/IEC 9075:1992 §11.21] a domain's <data type> is no domain, as {} is", name.ident);
                    return Err(rule(msg).or_at(name.at));
                }
                if self.catalog.user_type(&qn).is_some() {
                    let msg = format!("[ISO/IEC 9075-2:2023 §11.34] a domain's data type is a <predefined type>, not type {qn}");
                    return Err(rule(msg).or_at(name.at));
                }
                self.extension_type((name, &[]), missing)
            }
            TypeName::Extension { name, params } => self.extension_type((name, params), missing),
            TypeName::Array(..) | TypeName::Row(_) => Err(rule(
                "[ISO/IEC 9075-2:2023 §11.34] a domain's data type is a <predefined type>, and names no type as an element or a field"
                    .to_owned(),
            )),
        }
    }

    /// \[EXTENSION\] The type beyond the standard `name` with `params`,
    /// registered (RULES.md D53); else refused, `missing` the message.
    fn extension_type(&self, (name, params): (&TableName, &[u32]), missing: &str) -> Result<DataType> {
        let kind = self
            .types
            .kind(&name.ident)
            .filter(|_| name.auth.is_none())
            .ok_or_else(|| rule(format!("{missing} {}", name.ident)).or_at(name.at))?;
        kind.check_params(params).or_at(name.at)?;
        let params = TypeParams::of(params)
            .ok_or_else(|| rule(format!("[DESIGN] the type {} takes at most {MAX_PARAMS} parameters", name.ident)))?;
        Ok(DataType::Extension(ExtensionType { kind, params }))
    }
}
