//! Structured types and what is defined for user-defined types, as the
//! catalog describes them [ISO/IEC 9075-2:2023 §4.9.3, §4.9.4, §4.10,
//! §4.13, §4.36]: a structured type's attributes, method specifications
//! and reference representation; user-defined casts, orderings and
//! transforms; and what makes a table or view typed (RULES.md D81).
//!
//! A structured type's attributes are listed inherited first, in its
//! supertype's order, so an attribute's position is the same in every
//! subtype, as a value's attributes are kept (`value::types::Structured`).

use std::collections::BTreeSet;

use super::domains::{UsageDesc, usage_held};
use super::user_types::RoutineDesc;
use super::{QualifiedName, SqlParameter, Who};
use crate::ast::{DataAccess, MethodKind, OrderingForm, RefGeneration, TransformKind};
use crate::value::types::UdtName;
use crate::value::{DataType, types};

/// The suffix of the specific name of an attribute's observer function,
/// after its type's name and its own, joined by underscores (RULES.md
/// D81).
pub const OBSERVER_SUFFIX: &str = "_OBSERVER";
/// The suffix of the specific name of an attribute's mutator function.
pub const MUTATOR_SUFFIX: &str = "_MUTATOR";
/// The suffix of the specific name of a structured type's constructor
/// function, after its type's name.
pub const CONSTRUCTOR_SUFFIX: &str = "_CONSTRUCTOR";

/// \[DESIGN\] The most supertypes a type, or supertables a table, may
/// have: what bounds a walk up a hierarchy (RULES.md D81).
pub const MAX_HIERARCHY_DEPTH: usize = 256;

/// [ISO/IEC 9075-2:2023 §4.48.1, §19.13] The transform group names of an
/// SQL-session context: its default, and those set for user-defined types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransformNames {
    /// The default transform group's name, empty if none is set.
    pub default: String,
    /// Each type a group is set for, and the group's name, the latest set
    /// last.
    pub for_type: Vec<(QualifiedName, String)>,
}

impl TransformNames {
    /// [ISO/IEC 9075-2:2023 §6.4 GR21, §19.9 GR1b] The group name of the
    /// type `udt`, of proper supertypes `supertypes`: the one set last for
    /// it or a supertype, as one set for a type is set for each subtype;
    /// else the default group name.
    pub fn for_udt(&self, udt: &QualifiedName, supertypes: &[UdtName]) -> &str {
        let of = |t: &QualifiedName| t == udt || supertypes.iter().any(|s| s.schema == t.auth && s.name == t.ident);
        self.for_type.iter().rev().find(|(t, _)| of(t)).map_or(self.default.as_str(), |(_, g)| g.as_str())
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.3] A structured type's descriptor: what
/// §11.51 GR1 and GR3 make, as ALTER TYPE changes it.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredDesc {
    /// The `<user-defined type name>`.
    pub name: QualifiedName,
    /// Its direct supertype, if it has one.
    pub supertype: Option<QualifiedName>,
    /// Its proper supertypes, nearest first: what its declared type
    /// holds (`value::types::StructuredType`).
    pub supertypes: Vec<UdtName>,
    /// INSTANTIABLE, rather than NOT INSTANTIABLE.
    pub instantiable: bool,
    /// FINAL, rather than NOT FINAL.
    pub is_final: bool,
    /// Its attributes, inherited first, each its descriptor.
    pub attributes: Vec<AttributeDesc>,
    /// Its own method specifications, original and overriding, in order:
    /// not those it inherits.
    pub methods: Vec<MethodSpecDesc>,
    /// How the values of a reference type of it are made.
    pub reference: RefRepresentation,
    /// Of a user-defined representation, the names of its cast functions
    /// to the reference type \[SR8e\] and from it \[SR8f\].
    pub ref_casts: Option<(String, String)>,
    /// Of an instantiable type, its constructor function \[GR3\].
    pub constructor: Option<RoutineDesc>,
    /// USAGE on it [§12.3]: each grant.
    pub usage: Vec<UsageDesc>,
    /// UNDER on it [§12.3]: each grant, which a subtype's definer holds.
    pub under: Vec<UsageDesc>,
}

/// [ISO/IEC 9075-2:2023 §4.9.3.1] An attribute's descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeDesc {
    /// Its name.
    pub name: String,
    /// Its declared type.
    pub data_type: DataType,
    /// Its `<attribute default>` as written, if it has one.
    pub default: Option<String>,
    /// The collation of its `<collate clause>`, if written.
    pub collation: Option<QualifiedName>,
    /// The type that defines it: itself, or a proper supertype, of which
    /// it is inherited.
    pub defined_by: QualifiedName,
    /// Its observer function [§4.9.3.2], the defining type's.
    pub observer: RoutineDesc,
    /// Its mutator function [§4.9.3.3], the defining type's.
    pub mutator: RoutineDesc,
}

/// [ISO/IEC 9075-2:2023 §4.9.3.1] How the values of a reference type of a
/// structured type are made.
#[derive(Debug, Clone, PartialEq)]
pub enum RefRepresentation {
    /// System-generated: the number its typed table hierarchy gives each
    /// row inserted.
    System,
    /// User-defined: the values of its representation type, a predefined
    /// type.
    User(DataType),
    /// Derived: the values of these attributes.
    Derived(Vec<String>),
}

impl RefRepresentation {
    /// Its key words, as the Information Schema's USER_DEFINED_TYPES shows
    /// them in REFERENCE_TYPE.
    pub const fn name(&self) -> &'static str {
        match self {
            RefRepresentation::System => "SYSTEM GENERATED",
            RefRepresentation::User(_) => "USER DEFINED",
            RefRepresentation::Derived(_) => "DERIVED",
        }
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.4] A method specification's descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSpecDesc {
    /// The `<method name>`.
    pub name: String,
    /// Its specific name, in its type's schema, which the SQL-invoked
    /// method that gives its body has too.
    pub specific: QualifiedName,
    /// INSTANCE, STATIC or CONSTRUCTOR.
    pub kind: MethodKind,
    /// The specific name of the original method specification it
    /// overrides, a proper supertype's, if it is an overriding one.
    pub overrides: Option<QualifiedName>,
    /// Its parameters, the unaugmented list: SELF is not one.
    pub parameters: Vec<SqlParameter>,
    /// Its result type.
    pub returns: DataType,
    /// SELF AS RESULT.
    pub self_as_result: bool,
    /// DETERMINISTIC.
    pub deterministic: bool,
    /// Its SQL-data access indication.
    pub access: DataAccess,
    /// RETURNS NULL ON NULL INPUT.
    pub null_call: bool,
    /// Its order of creation, as a routine's (`SqlRoutineDesc::created`).
    pub created: u64,
    /// Its creation and last-altered timestamps [§4.9.7]: those of the
    /// statement that specified it [§11.51 GR1g xv, xvi, GR1h xiv, xv;
    /// §11.56 GR1o, GR1p].
    pub stamps: super::Stamps,
}

/// [ISO/IEC 9075-2:2023 §4.10] A user-defined cast's descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct CastDesc {
    /// The source type.
    pub source: DataType,
    /// The target type.
    pub target: DataType,
    /// The specific name of its cast function.
    pub function: QualifiedName,
    /// AS ASSIGNMENT: it is an implicit cast in store and retrieval
    /// assignments [§9.1, §9.2].
    pub assignment: bool,
}

/// [ISO/IEC 9075-2:2023 §4.13] A user-defined ordering's category, and
/// its function's specific name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderingCategory {
    /// RELATIVE: a function of two values giving a negative number, 0 or a
    /// positive one.
    Relative(QualifiedName),
    /// MAP: a function of a value giving one of a predefined type that
    /// compares in its place.
    Map(QualifiedName),
    /// STATE: the values compare by their most specific types and their
    /// attributes, in order; the name of its function, if written.
    State(Option<QualifiedName>),
}

/// [ISO/IEC 9075-2:2023 §4.13] A user-defined type's ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingDesc {
    /// The type.
    pub udt: QualifiedName,
    /// EQUALS ONLY or FULL.
    pub form: OrderingForm,
    /// Its category and function.
    pub category: OrderingCategory,
}

impl OrderingDesc {
    /// The specific name of its ordering function, if it has one.
    pub fn function(&self) -> Option<&QualifiedName> {
        match &self.category {
            OrderingCategory::Relative(f) | OrderingCategory::Map(f) => Some(f),
            OrderingCategory::State(f) => f.as_ref(),
        }
    }

    /// Its category's key word, as the Information Schema shows it.
    pub const fn category_name(&self) -> &'static str {
        match self.category {
            OrderingCategory::Relative(_) => "RELATIVE",
            OrderingCategory::Map(_) => "MAP",
            OrderingCategory::State(_) => "STATE",
        }
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.5] A transform group of a user-defined type:
/// its name and the specific names of its functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformGroupDesc {
    /// The `<group name>`.
    pub name: String,
    /// Its to-sql function, if it has one.
    pub to_sql: Option<QualifiedName>,
    /// Its from-sql function, if it has one.
    pub from_sql: Option<QualifiedName>,
}

impl TransformGroupDesc {
    /// Its function of kind `kind`, if it has one.
    pub fn function(&self, kind: TransformKind) -> Option<&QualifiedName> {
        match kind {
            TransformKind::ToSql => self.to_sql.as_ref(),
            TransformKind::FromSql => self.from_sql.as_ref(),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §4.15.3, §4.16] What makes a table or a view
/// typed: its structured type, its self-referencing column, the first,
/// and its direct supertable.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedDesc {
    /// The structured type.
    pub udt: QualifiedName,
    /// The self-referencing column's name.
    pub self_referencing: String,
    /// How its values are made.
    pub generation: RefGeneration,
    /// Its direct supertable, or of a view its direct superview.
    pub supertable: Option<QualifiedName>,
}

impl StructuredDesc {
    /// The structured type it describes, as a declared type.
    pub fn data_type(&self) -> DataType {
        DataType::structured(UdtName::new(&self.name.auth, &self.name.ident), self.supertypes.clone())
    }

    /// The structured type it describes, as a referenced type.
    pub fn structured_type(&self) -> types::StructuredType {
        types::StructuredType { name: UdtName::new(&self.name.auth, &self.name.ident), supertypes: self.supertypes.clone() }
    }

    /// The attribute named `name`, and its position, if it has one.
    pub fn attribute(&self, name: &str) -> Option<(usize, &AttributeDesc)> {
        self.attributes.iter().enumerate().find(|(_, a)| a.name == name)
    }

    /// Whether `who` holds USAGE on it, and whether grantably, its schema's
    /// owner `owner` holding it grantably.
    pub fn usage_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, who, owner)
    }

    /// Whether `who` holds UNDER on it, and whether grantably.
    pub fn under_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.under, who, owner)
    }

    /// Whether it is a proper subtype of `of`.
    pub fn is_proper_subtype_of(&self, of: &QualifiedName) -> bool {
        self.supertypes.iter().any(|s| s.schema == of.auth && s.name == of.ident)
    }

    /// Its maximal supertype: the last of its proper supertypes, or itself.
    pub fn maximal(&self) -> QualifiedName {
        self.supertypes.last().map_or_else(|| self.name.clone(), |s| QualifiedName { auth: s.schema.clone(), ident: s.name.clone() })
    }
}

/// The qualified name of the type `n` names.
pub fn udt_qualified(n: &UdtName) -> QualifiedName {
    QualifiedName { auth: n.schema.clone(), ident: n.name.clone() }
}

/// [ISO/IEC 9075-2:2023 §4.10, §11.51 SR9] A user-defined type that may
/// have method specifications: a structured type, or a distinct type,
/// which has no attributes, supertypes or subtypes.
#[derive(Debug, Clone, Copy)]
pub enum MethodHost<'a> {
    /// A structured type.
    Structured(&'a StructuredDesc),
    /// A distinct type.
    Distinct(&'a super::UserTypeDesc),
}

impl<'a> MethodHost<'a> {
    /// The type's name.
    pub fn name(self) -> &'a QualifiedName {
        match self {
            MethodHost::Structured(s) => &s.name,
            MethodHost::Distinct(d) => &d.name,
        }
    }

    /// Its own method specifications.
    pub fn methods(self) -> &'a [MethodSpecDesc] {
        match self {
            MethodHost::Structured(s) => &s.methods,
            MethodHost::Distinct(d) => &d.methods,
        }
    }

    /// Its proper supertypes, nearest first.
    pub fn supertypes(self) -> &'a [UdtName] {
        match self {
            MethodHost::Structured(s) => &s.supertypes,
            MethodHost::Distinct(_) => &[],
        }
    }

    /// The type, as a declared type: SELF's.
    pub fn data_type(self) -> DataType {
        match self {
            MethodHost::Structured(s) => s.data_type(),
            MethodHost::Distinct(d) => d.data_type(),
        }
    }

    /// The structured type it is, if it is one.
    pub fn structured(self) -> Option<&'a StructuredDesc> {
        match self {
            MethodHost::Structured(s) => Some(s),
            MethodHost::Distinct(_) => None,
        }
    }
}

/// The user-defined type `qn` of `catalog` that may have methods, if
/// there is one.
pub fn method_host<'a>(catalog: &'a dyn super::Catalog, qn: &QualifiedName) -> Option<MethodHost<'a>> {
    catalog.structured_type(qn).map(MethodHost::Structured).or_else(|| catalog.user_type(qn).map(MethodHost::Distinct))
}

/// [ISO/IEC 9075-2:2023 §4.17.9] The proper subtables of the table `t` in
/// `catalog`, each before its own proper subtables, the direct subtables
/// of each in the order they were made: at most `MAX_HIERARCHY_DEPTH`
/// deep, as a table hierarchy is. Of a referenceable view, its proper
/// subviews [§11.32 SR22].
pub fn proper_subtables(catalog: &dyn super::Catalog, t: &QualifiedName) -> Vec<QualifiedName> {
    let (tables, views) = (catalog.tables(), catalog.views());
    let typed = tables.iter().map(|d| (&d.name, d.typed.as_ref())).chain(views.iter().map(|v| (&v.name, v.typed.as_ref())));
    let members: Vec<(&QualifiedName, Option<&QualifiedName>)> =
        typed.map(|(n, ty)| (n, ty.and_then(|ty| ty.supertable.as_ref()))).collect();
    let direct =
        |of: &QualifiedName| -> Vec<QualifiedName> { members.iter().filter(|(_, s)| *s == Some(of)).map(|(n, _)| (*n).clone()).collect() };
    let mut out = Vec::new();
    let mut stack: Vec<(QualifiedName, usize)> = direct(t).into_iter().rev().map(|n| (n, 1)).collect();
    while let Some((n, depth)) = stack.pop() {
        if depth <= MAX_HIERARCHY_DEPTH {
            stack.extend(direct(&n).into_iter().rev().map(|s| (s, depth.saturating_add(1))));
            out.push(n);
        }
    }
    out
}

/// [ISO/IEC 9075-2:2023 §4.17.9] The proper supertables of the table `t`
/// in `catalog`, the direct supertable first, at most
/// `MAX_HIERARCHY_DEPTH`: of a referenceable view, its proper superviews
/// [§11.32 SR22].
pub fn proper_supertables(catalog: &dyn super::Catalog, t: &QualifiedName) -> Vec<QualifiedName> {
    let above = |n: &QualifiedName| {
        let typed = catalog.table(n).and_then(|d| d.typed.as_ref()).or_else(|| catalog.view(n).and_then(|v| v.typed.as_ref()));
        typed.and_then(|ty| ty.supertable.clone())
    };
    let mut out: Vec<QualifiedName> = Vec::new();
    let mut at = above(t);
    while let Some(s) = at.filter(|_| out.len() < MAX_HIERARCHY_DEPTH) {
        at = above(&s);
        out.push(s);
    }
    out
}

/// [ISO/IEC 9075-2:2023 §4.42.2, §11.3 GR7] The specific names of the
/// methods of the structured type `udt` in `catalog`: those of it and of
/// its proper supertypes, an attribute's observer and mutator among them,
/// each a method on whose table/method pair a privilege may be held.
pub fn type_methods(catalog: &dyn super::Catalog, udt: &QualifiedName) -> BTreeSet<QualifiedName> {
    let Some(t) = catalog.structured_type(udt) else { return BTreeSet::new() };
    let types = std::iter::once(t).chain(t.supertypes.iter().filter_map(|s| catalog.structured_type(&udt_qualified(s))));
    let mut all = BTreeSet::new();
    for desc in types {
        let named = |ident: &str| QualifiedName { auth: desc.name.auth.clone(), ident: ident.to_owned() };
        let own = desc.attributes.iter().filter(|a| a.defined_by == desc.name);
        all.extend(own.flat_map(|a| [named(&a.observer.specific), named(&a.mutator.specific)]));
        all.extend(desc.methods.iter().map(|m| m.specific.clone()));
    }
    all
}

/// [ISO/IEC 9075-2:2023 §12.2 GR6] Whether `grantor` holds EXECUTE WITH
/// GRANT OPTION on the method of specific name `specific`: as the owner of
/// its type's schema, or by a grant of its observer's, mutator's or
/// SQL-invoked method's EXECUTE.
pub fn method_executable(catalog: &dyn super::Catalog, specific: &QualifiedName, grantor: &str) -> bool {
    if super::owner_of(catalog, &specific.auth) == grantor {
        return true;
    }
    let types = catalog.structured_types();
    let implicit =
        types.iter().filter(|t| t.name.auth == specific.auth).flat_map(|t| t.attributes.iter()).flat_map(|a| [&a.observer, &a.mutator]);
    let execute = match implicit.into_iter().find(|r| r.specific == specific.ident) {
        Some(r) => r.execute.as_slice(),
        None => catalog.routine(specific).map_or(&[][..], |r| r.execute.as_slice()),
    };
    execute.iter().any(|u| u.grantable && u.grantee.as_deref() == Some(grantor))
}
