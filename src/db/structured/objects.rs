//! What the schema statements of user-defined types name: the object each
//! defines or alters, as its Access Rule asks the session to own its
//! schema; what a DROP of a cast, ordering or transform drops; a reference
//! type [ISO/IEC 9075-2:2023 §6.1]; and the generated columns a drop loses.

use std::sync::Arc;

use super::super::generated::Column;
use super::super::{Session, State, rule};
use crate::ast::{DropObject, MethodSignature, SchemaStatement, TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, Who, owner_of};
use crate::error::{At, Code, Result, SqlError};
use crate::log::record::DropTarget;
use crate::value::DataType;
use crate::value::types::UdtName;

/// The type the user-defined type schema statement `statement` defines or
/// alters what is of, as written, if it names one, and the Access Rule of
/// its schema's owner.
pub(in crate::db) fn type_named(statement: &SchemaStatement) -> (Option<&TableName>, &'static str) {
    match statement {
        SchemaStatement::AlterType(a) => (Some(&a.name), "§11.53 AR1"),
        SchemaStatement::Method(m) => match &m.signature {
            MethodSignature::Signature { udt, .. } => (Some(udt), "§11.60 AR1"),
            MethodSignature::Specific(n) => (Some(n), "§11.60 AR1"),
        },
        SchemaStatement::Cast(c) => (type_written(&c.source).or_else(|| type_written(&c.target)), "§11.63 AR2"),
        SchemaStatement::Ordering(o) => (Some(&o.udt), "§11.65 AR1"),
        SchemaStatement::Transform(t) => (Some(&t.udt), "§11.67 AR1"),
        SchemaStatement::AlterTransform(t) => (Some(&t.udt), "§11.68 AR1"),
        SchemaStatement::Table(_)
        | SchemaStatement::View(_)
        | SchemaStatement::Grant(_)
        | SchemaStatement::Revoke(_)
        | SchemaStatement::Role(_)
        | SchemaStatement::Domain(_)
        | SchemaStatement::Assertion(_)
        | SchemaStatement::Collation(_)
        | SchemaStatement::Sequence(_)
        | SchemaStatement::AlterSequence(_)
        | SchemaStatement::AlterDomain(_)
        | SchemaStatement::Type(_)
        | SchemaStatement::Trigger(_)
        | SchemaStatement::Routine(_)
        | SchemaStatement::Module(_)
        | SchemaStatement::Graph(_)
        | SchemaStatement::AlterGraph(_) => (None, "§11.53 AR1"),
    }
}

impl Session {
    /// The user-defined type schema statement `statement` defines or
    /// alters what is of, its names in schema `schema`, in a schema the
    /// session owns, as its Access Rule says.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the session does not own the type's schema [§11.53
    /// AR1, §11.60 AR1, §11.63 AR2, §11.65 AR1, §11.67 AR1, §11.68 AR1].
    pub(in crate::db) fn type_object(&self, st: &State, statement: &SchemaStatement, schema: &str) -> Result<QualifiedName> {
        let (name, cited) = type_named(statement);
        let Some(name) = name else { return Ok(QualifiedName::of_schema(schema)) };
        let mut qn = qualify(name, schema);
        // A method by its specific name is of the type that has it.
        if let SchemaStatement::Method(m) = statement
            && let MethodSignature::Specific(_) = &m.signature
        {
            let structured =
                st.catalog.structured_types().into_iter().find(|t| t.methods.iter().any(|s| s.specific == qn)).map(|t| t.name.clone());
            let of = structured.or_else(|| {
                st.catalog.user_types().into_iter().find(|t| t.methods.iter().any(|s| s.specific == qn)).map(|t| t.name.clone())
            });
            qn = of.unwrap_or(qn);
        }
        self.owning(st, &qn, (name, cited))
    }

    /// `qn`, if the session owns its schema, as the Access Rule `cited`
    /// says, `name` where it is written.
    fn owning(&self, st: &State, qn: &QualifiedName, (name, cited): (&TableName, &str)) -> Result<QualifiedName> {
        if st.catalog.schema_exists(&qn.auth) && !self.owns_schema(st, &qn.auth) {
            let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {qn} is in schema {}, which {} does not own", qn.auth, self.auth());
            return Err(SqlError::new(Code::Privilege, msg).or_at(name.at));
        }
        Ok(qn.clone())
    }

    /// What DROP of `object` names, a cast, ordering or transform, in a
    /// schema the session owns.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if it does not own the schema [§11.64 AR1, §11.66 AR1,
    /// §11.71 AR1]; a type of a cast's, named.
    pub(in crate::db) fn type_drop_target(&self, st: &State, object: &DropObject) -> Result<DropTarget> {
        let schema = self.default_schema();
        match object {
            DropObject::Cast(types, at) => {
                let authorization = self.authorization();
                let who = authorization.of(st.catalog.roles()).cell;
                let source = st.resolve_type(&types.0, (&schema, who)).or_at(*at)?.0;
                let target = st.resolve_type(&types.1, (&schema, who)).or_at(*at)?.0;
                let cast = st.catalog.casts().into_iter().find(|c| c.source == source && c.target == target).cloned();
                if let Some(c) = cast
                    && !self.owns_schema(st, &c.function.auth)
                {
                    let msg = format!(
                        "[ISO/IEC 9075-2:2023 §11.64 AR1] the cast's function {} is in a schema {} does not own",
                        c.function,
                        self.auth()
                    );
                    return Err(SqlError::new(Code::Privilege, msg).or_at(*at));
                }
                Ok(DropTarget::Cast(Box::new((source, target))))
            }
            DropObject::Ordering(name) => Ok(DropTarget::Ordering(self.owning(st, &qualify(name, &schema), (name, "§11.66 AR1"))?)),
            DropObject::Transform(name, group) => {
                Ok(DropTarget::Transform(self.owning(st, &qualify(name, &schema), (name, "§11.71 AR1"))?, group.clone()))
            }
            DropObject::Table(name)
            | DropObject::View(name)
            | DropObject::Domain(name)
            | DropObject::Assertion(name)
            | DropObject::Collation(name)
            | DropObject::Sequence(name)
            | DropObject::Type(name)
            | DropObject::Trigger(name)
            | DropObject::Module(name)
            | DropObject::Graph(name, _) => Err(rule(format!("[DESIGN] {} is no cast, ordering or transform", name.ident))),
            DropObject::Routine(_) | DropObject::Schema { .. } => Err(rule("[DESIGN] no cast, ordering or transform is named")),
        }
    }
}

/// The name of the user-defined type, or of a reference type's referenced
/// type, `t` writes, if it writes one.
fn type_written(t: &TypeName) -> Option<&TableName> {
    match t {
        TypeName::Domain(name) | TypeName::Reference(name, _) => Some(name),
        TypeName::Data(_) | TypeName::Collection(..) | TypeName::Row(_) | TypeName::Extension { .. } => None,
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §6.1 SR44, SR45, GR24, AR1] REF(`referenced`)
    /// SCOPE `scope`, their names in schema `schema`, for the cell `who`:
    /// of a structured type `who` holds USAGE on, its scope a typed table
    /// or view of that type.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `referenced` names no structured type \[SR44\], or
    /// `scope` no typed table or view of it \[SR45\]; -104 if `who` holds no
    /// USAGE on the type \[AR1\].
    pub(in crate::db) fn reference_type(
        &self,
        (referenced, scope): (&TableName, Option<&TableName>),
        (schema, who): (&str, Who<'_>),
    ) -> Result<DataType> {
        let rq = qualify(referenced, schema);
        let Some(desc) = self.catalog.structured_type(&rq) else {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §6.1 SR44] {rq} is no structured type, as a referenced type is")).or_at(referenced.at)
            );
        };
        if !desc.usage_of(who, owner_of(&*self.catalog, &rq.auth)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §6.1 AR1] {} holds no USAGE on type {rq}", who.auth);
            return Err(SqlError::new(Code::Privilege, msg).or_at(referenced.at));
        }
        let Some(s) = scope else { return Ok(DataType::reference(desc.structured_type(), None)) };
        let sq = qualify(s, schema);
        let typed =
            self.catalog.table(&sq).and_then(|t| t.typed.as_ref()).or_else(|| self.catalog.view(&sq).and_then(|v| v.typed.as_ref()));
        if typed.is_none_or(|t| t.udt != rq) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.1 SR45] the scope {sq} is no typed table of type {rq}")).or_at(s.at));
        }
        Ok(DataType::reference(desc.structured_type(), Some(UdtName::new(&sq.auth, &sq.ident))))
    }

    /// The generated columns whose generation expressions no longer bind
    /// without what `target`, a cast, ordering or transform, drops: as
    /// DROP of it with CASCADE loses them first.
    pub(in crate::db) fn generations_without(&self, target: &DropTarget) -> Vec<Column> {
        let mut catalog = self.catalog.duplicate();
        match target {
            DropTarget::Cast(types) => catalog.remove_cast(&types.0, &types.1),
            DropTarget::Ordering(qn) => catalog.remove_ordering(qn),
            // [§4.9.6] A transform applies at no expression's binding.
            DropTarget::Transform(..)
            | DropTarget::Table(_)
            | DropTarget::View(_)
            | DropTarget::Schema(_)
            | DropTarget::Domain(_)
            | DropTarget::Assertion(_)
            | DropTarget::Collation(_)
            | DropTarget::Sequence(_)
            | DropTarget::Type(_)
            | DropTarget::Trigger(_)
            | DropTarget::Routine(_)
            | DropTarget::Module(_)
            | DropTarget::Graph(_) => return Vec::new(),
        }
        let mut after = self.clone();
        after.catalog = Arc::from(catalog);
        self.unbound_in(&after, &|_| true, &|_| true).columns
    }
}
