//! Abstract syntax of structured types, their methods, and what is defined
//! for a user-defined type [ISO/IEC 9075-2:2023 §11.51 to §11.58, §11.61],
//! as written: attributes, method specifications and SQL-invoked methods,
//! ALTER TYPE, user-defined casts, orderings and transforms, and SET
//! TRANSFORM GROUP [§19.13] (RULES.md D81).

use super::{DefaultOption, ParameterDeclaration, RoutineBody, RoutineDesignator, TableName, TypeName};
use crate::ast::DataAccess;

/// [ISO/IEC 9075-2:2023 §11.51] `<attribute definition>` ::= `<attribute
/// name>` `<data type>` \[`<attribute default>`\] \[`<collate clause>`\]
#[derive(Debug, Clone)]
pub struct AttributeDefinition {
    /// The `<attribute name>`.
    pub name: String,
    /// The `<data type>`.
    pub data_type: TypeName,
    /// The `<default option>` of its `<attribute default>`, if written.
    pub default: Option<DefaultOption>,
    /// The `<attribute default>` as written, which the descriptor keeps.
    pub default_text: Option<String>,
    /// The `<collation name>` of its `<collate clause>`, if written.
    pub collate: Option<TableName>,
    /// Where it begins.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §11.51] `<reference type specification>`: how
/// the values of a reference type of it are made.
#[derive(Debug, Clone)]
pub enum RefRepresentation {
    /// `<system-generated representation>` ::= REF IS SYSTEM GENERATED,
    /// the default \[SR8\].
    System,
    /// `<user-defined representation>` ::= REF USING `<predefined type>`.
    User(TypeName),
    /// `<derived representation>` ::= REF FROM `<list of attributes>`.
    Derived(Vec<String>),
}

/// [ISO/IEC 9075-2:2023 §11.52] The kind of a method: INSTANCE, the
/// default, STATIC or CONSTRUCTOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// An instance SQL-invoked method, of SELF.
    Instance,
    /// A static SQL-invoked method, of no instance.
    Static,
    /// A constructor, an instance method named as its type, that gives
    /// SELF.
    Constructor,
}

impl MethodKind {
    /// Its key word, as the Information Schema shows it.
    pub const fn name(self) -> &'static str {
        match self {
            MethodKind::Instance => "INSTANCE",
            MethodKind::Static => "STATIC",
            MethodKind::Constructor => "CONSTRUCTOR",
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.51] `<method specification>` ::= `<original
/// method specification>` | `<overriding method specification>`
#[derive(Debug, Clone)]
pub struct MethodSpecification {
    /// OVERRIDING was written.
    pub overriding: bool,
    /// INSTANCE, STATIC or CONSTRUCTOR.
    pub kind: MethodKind,
    /// The `<method name>`.
    pub name: String,
    /// The `<SQL parameter declaration>`s, in order: the unaugmented list.
    pub parameters: Vec<ParameterDeclaration>,
    /// The `<returns data type>`.
    pub returns: TypeName,
    /// SELF AS RESULT was written.
    pub self_as_result: bool,
    /// The `<specific method name>` of SPECIFIC, if written.
    pub specific: Option<TableName>,
    /// The `<method characteristics>`, of an original specification.
    pub characteristics: MethodCharacteristics,
    /// Where it begins.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §11.51] `<method characteristics>`, each as
/// written or its default \[SR8\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodCharacteristics {
    /// DETERMINISTIC was written.
    pub deterministic: bool,
    /// The `<SQL-data access indication>`: CONTAINS SQL by default.
    pub access: DataAccess,
    /// RETURNS NULL ON NULL INPUT was written.
    pub null_call: bool,
}

impl Default for MethodCharacteristics {
    fn default() -> Self {
        MethodCharacteristics { deterministic: false, access: DataAccess::Contains, null_call: false }
    }
}

/// [ISO/IEC 9075-2:2023 §11.53] `<alter type statement>` ::= ALTER TYPE
/// `<schema-resolved user-defined type name>` `<alter type action>`
#[derive(Debug, Clone)]
pub struct AlterType {
    /// The type.
    pub name: TableName,
    /// The `<alter type action>`.
    pub action: AlterTypeAction,
}

/// [ISO/IEC 9075-2:2023 §11.53] `<alter type action>`.
#[derive(Debug, Clone)]
pub enum AlterTypeAction {
    /// [§11.54] ADD ATTRIBUTE `<attribute definition>`.
    AddAttribute(AttributeDefinition),
    /// [§11.55] DROP ATTRIBUTE `<attribute name>` RESTRICT, and where it
    /// is written.
    DropAttribute(String, usize),
    /// [§11.56] ADD `<method specification>`.
    AddMethod(MethodSpecification),
    /// [§11.57] DROP `<specific method specification designator>`
    /// RESTRICT.
    DropMethod(MethodDesignator),
}

/// [ISO/IEC 9075-2:2023 §11.57] `<specific method specification
/// designator>` ::= \[INSTANCE | STATIC | CONSTRUCTOR\] METHOD `<method
/// name>` `<data type list>`
#[derive(Debug, Clone)]
pub struct MethodDesignator {
    /// The kind written, INSTANCE if none.
    pub kind: MethodKind,
    /// The `<method name>`, and where it is written.
    pub name: (String, usize),
    /// The `<data type list>`.
    pub parameters: Vec<TypeName>,
}

/// [ISO/IEC 9075-2:2023 §11.61] `<method specification designator>`:
/// which method specification a CREATE METHOD gives the body of.
#[derive(Debug, Clone)]
pub enum MethodSignature {
    /// SPECIFIC METHOD `<specific method name>`.
    Specific(TableName),
    /// \[INSTANCE | STATIC | CONSTRUCTOR\] METHOD `<method name>` `<SQL
    /// parameter declaration list>` \[`<returns clause>`\] FOR
    /// `<schema-resolved user-defined type name>`.
    Signature {
        /// The kind written, INSTANCE if none.
        kind: MethodKind,
        /// The `<method name>`, and where it is written.
        name: (String, usize),
        /// The parameters.
        parameters: Vec<ParameterDeclaration>,
        /// The `<returns data type>`, if written.
        returns: Option<TypeName>,
        /// SELF AS RESULT was written.
        self_as_result: bool,
        /// The type.
        udt: TableName,
    },
}

/// [ISO/IEC 9075-2:2023 §11.60, §11.61] `<SQL-invoked method>` ::=
/// CREATE `<method specification designator>` `<routine body>`
#[derive(Debug, Clone)]
pub struct MethodDefinition {
    /// The method specification it gives the body of.
    pub signature: MethodSignature,
    /// Its `<routine body>`.
    pub body: RoutineBody,
    /// The statement as written, which the descriptor keeps to bind the
    /// body again.
    pub text: String,
    /// The `<SQL routine body>` as written.
    pub body_text: String,
    /// Where it begins.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §11.63] `<user-defined cast definition>` ::=
/// CREATE CAST ( `<source data type>` AS `<target data type>` ) WITH
/// `<cast function>` \[AS ASSIGNMENT\]
#[derive(Debug, Clone)]
pub struct CastDefinition {
    /// The source type.
    pub source: TypeName,
    /// The target type.
    pub target: TypeName,
    /// The `<specific routine designator>` of the cast function.
    pub function: RoutineDesignator,
    /// AS ASSIGNMENT was written.
    pub assignment: bool,
    /// Where it begins.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §11.65] `<ordering form>`: EQUALS ONLY or ORDER
/// FULL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingForm {
    /// EQUALS ONLY: only equality.
    Equals,
    /// ORDER FULL: every comparison.
    Full,
}

/// [ISO/IEC 9075-2:2023 §11.65] `<ordering category>`.
#[derive(Debug, Clone)]
pub enum OrderingCategory {
    /// RELATIVE WITH `<relative function specification>`.
    Relative(RoutineDesignator),
    /// MAP WITH `<map function specification>`.
    Map(RoutineDesignator),
    /// STATE \[`<specific name>`\].
    State(Option<TableName>),
}

/// [ISO/IEC 9075-2:2023 §11.65] `<user-defined ordering definition>` ::=
/// CREATE ORDERING FOR `<schema-resolved user-defined type name>`
/// `<ordering form>`
#[derive(Debug, Clone)]
pub struct OrderingDefinition {
    /// The type.
    pub udt: TableName,
    /// EQUALS ONLY or ORDER FULL.
    pub form: OrderingForm,
    /// BY its category.
    pub category: OrderingCategory,
}

/// [ISO/IEC 9075-2:2023 §11.67] Which function of a transform group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformKind {
    /// TO SQL.
    ToSql,
    /// FROM SQL.
    FromSql,
}

/// [ISO/IEC 9075-2:2023 §11.67] `<transform group>` ::= `<group name>` (
/// `<transform element list>` )
#[derive(Debug, Clone)]
pub struct TransformGroup {
    /// The `<group name>`, and where it is written.
    pub name: (String, usize),
    /// Its `<transform element>`s: each function's kind and designator.
    pub elements: Vec<(TransformKind, RoutineDesignator)>,
}

/// [ISO/IEC 9075-2:2023 §11.67] `<transform definition>` ::= CREATE
/// {TRANSFORM | TRANSFORMS} FOR `<schema-resolved user-defined type name>`
/// `<transform group>`...
#[derive(Debug, Clone)]
pub struct TransformDefinition {
    /// The type.
    pub udt: TableName,
    /// The groups, one or more.
    pub groups: Vec<TransformGroup>,
}

/// [ISO/IEC 9075-2:2023 §11.68] `<alter group>`: a group's name and what
/// is done to it.
#[derive(Debug, Clone)]
pub struct AlterGroup {
    /// The `<group name>`, and where it is written.
    pub name: (String, usize),
    /// ADD its elements [§11.69], or DROP the kinds named [§11.70].
    pub action: AlterGroupAction,
}

/// [ISO/IEC 9075-2:2023 §11.69, §11.70] `<alter transform action>`.
#[derive(Debug, Clone)]
pub enum AlterGroupAction {
    /// `<add transform element list>`.
    Add(Vec<(TransformKind, RoutineDesignator)>),
    /// `<drop transform element list>`, RESTRICT or CASCADE.
    Drop(Vec<TransformKind>, super::DropBehavior),
}

/// [ISO/IEC 9075-2:2023 §11.68] `<alter transform statement>` ::= ALTER
/// {TRANSFORM | TRANSFORMS} FOR `<schema-resolved user-defined type name>`
/// `<alter group>`...
#[derive(Debug, Clone)]
pub struct AlterTransform {
    /// The type.
    pub udt: TableName,
    /// The groups altered.
    pub groups: Vec<AlterGroup>,
}

/// [ISO/IEC 9075-2:2023 §19.13] `<transform group characteristic>`.
#[derive(Debug, Clone)]
pub enum TransformGroupSetting {
    /// DEFAULT TRANSFORM GROUP `<group name>`.
    Default(String),
    /// TRANSFORM GROUP FOR TYPE `<path-resolved user-defined type name>`
    /// `<group name>`.
    ForType(TableName, String),
}

/// [ISO/IEC 9075-2:2023 §6.17 to §6.24, §6.33, §6.4]
/// An expression of user-defined types and references: a `ValueExpression::
/// Structured`.
#[derive(Debug, Clone)]
pub enum StructuredExpression {
    /// [§6.18] `<method invocation>`: `<value expression primary>` .
    /// `<method name>` \[`<SQL argument list>`\], or a `<generalized
    /// invocation>` ( `<value expression primary>` AS `<data type>` ) .
    /// `<method name>` \[`<SQL argument list>`\].
    Method {
        /// The subject, the value of SELF.
        subject: super::ValueExpression,
        /// The `<data type>` of a `<generalized invocation>`.
        as_type: Option<TypeName>,
        /// The `<method name>`.
        name: String,
        /// The arguments, in order.
        args: Vec<super::ValueExpression>,
    },
    /// [§6.19] `<static method invocation>` ::= `<path-resolved user-
    /// defined type name>` :: `<method name>` \[`<SQL argument list>`\].
    Static {
        /// The type.
        udt: TableName,
        /// The `<method name>`.
        name: String,
        /// The arguments, in order.
        args: Vec<super::ValueExpression>,
    },
    /// [§6.20] `<new specification>` ::= NEW `<path-resolved user-defined
    /// type name>` `<SQL argument list>`.
    New {
        /// The type.
        udt: TableName,
        /// The arguments, in order.
        args: Vec<super::ValueExpression>,
    },
    /// [§6.17] `<subtype treatment>` ::= TREAT ( `<subtype operand>` AS
    /// `<target subtype>` ).
    Treat {
        /// The operand.
        subject: super::ValueExpression,
        /// The `<target subtype>`: a user-defined type, or a reference type.
        target: TypeName,
    },
    /// [§6.40] `<specific type method>` ::= `<user-defined type value
    /// expression>` . SPECIFICTYPE \[( )\].
    SpecificType(super::ValueExpression),
    /// [§6.22] `<dereference operation>` ::= `<reference value
    /// expression>` -> `<attribute name>`.
    Dereference {
        /// The reference.
        subject: super::ValueExpression,
        /// The `<attribute name>`.
        attribute: String,
    },
    /// [§6.23] `<method reference>` ::= `<value expression primary>` ->
    /// `<method name>` `<SQL argument list>`.
    MethodReference {
        /// The reference.
        subject: super::ValueExpression,
        /// The `<method name>`.
        name: String,
        /// The arguments, in order.
        args: Vec<super::ValueExpression>,
    },
    /// [§6.24] `<reference resolution>` ::= DEREF ( `<reference value
    /// expression>` ).
    Deref(super::ValueExpression),
    /// [§6.4] CURRENT_DEFAULT_TRANSFORM_GROUP.
    CurrentDefaultTransformGroup,
    /// [§6.4] CURRENT_TRANSFORM_GROUP_FOR_TYPE `<path-resolved user-defined
    /// type name>`.
    CurrentTransformGroupForType(TableName),
}

/// [ISO/IEC 9075-2:2023 §8.19] `<type predicate>` ::= `<row value
/// predicand>` IS \[NOT\] OF ( `<type list>` ): each type, ONLY if `.0`.
#[derive(Debug, Clone)]
pub struct TypeList(pub Vec<(bool, TypeName)>);

/// [ISO/IEC 9075-2:2023 §11.3] `<table definition>`'s `<typed table
/// clause>` ::= OF `<path-resolved user-defined type name>` \[`<subtable
/// clause>`\]; and [§11.32] a `<referenceable view specification>`'s.
#[derive(Debug, Clone)]
pub struct TypedTable {
    /// The structured type.
    pub udt: TableName,
    /// UNDER `<supertable name>`, if written.
    pub under: Option<TableName>,
}

/// [ISO/IEC 9075-2:2023 §11.3] `<reference generation>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefGeneration {
    /// SYSTEM GENERATED.
    System,
    /// USER GENERATED.
    User,
    /// DERIVED.
    Derived,
}

impl RefGeneration {
    /// Its key words, as the Information Schema shows them.
    pub const fn name(self) -> &'static str {
        match self {
            RefGeneration::System => "SYSTEM GENERATED",
            RefGeneration::User => "USER GENERATED",
            RefGeneration::Derived => "DERIVED",
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.3] `<column options>` ::= `<column name>`
/// WITH OPTIONS `<column option list>`, of an attribute's column.
#[derive(Debug, Clone)]
pub struct ColumnOptions {
    /// The `<column name>`, and where it is written.
    pub column: (String, usize),
    /// The `<scope clause>`'s table, if written.
    pub scope: Option<TableName>,
    /// The `<default option>` of its `<default clause>`, if written.
    pub default: Option<DefaultOption>,
    /// The `<column constraint definition>`s, in order.
    pub constraints: Vec<super::ColumnConstraint>,
}

impl StructuredExpression {
    /// Calls `f` with each value expression it is made of: its subject and
    /// its arguments.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a super::ValueExpression)) {
        match self {
            StructuredExpression::Method { subject, args, .. } | StructuredExpression::MethodReference { subject, args, .. } => {
                f(subject);
                args.iter().for_each(f);
            }
            StructuredExpression::Static { args, .. } | StructuredExpression::New { args, .. } => args.iter().for_each(f),
            StructuredExpression::Treat { subject, .. }
            | StructuredExpression::Dereference { subject, .. }
            | StructuredExpression::SpecificType(subject)
            | StructuredExpression::Deref(subject) => f(subject),
            StructuredExpression::CurrentDefaultTransformGroup | StructuredExpression::CurrentTransformGroupForType(_) => {}
        }
    }
}
