//! What is defined for a user-defined type: [ISO/IEC 9075-2:2023 §11.63,
//! §11.64] user-defined casts, [§11.65, §11.66] orderings, [§11.67 to
//! §11.71] transforms, each defined, altered or dropped, and [§19.13] the
//! `<set transform group statement>`.

use super::broken;
use crate::ast::{
    AlterGroup, AlterGroupAction, AlterTransform, CastDefinition, DropObject, OrderingCategory, OrderingDefinition, OrderingForm,
    RoutineDesignator, SchemaStatement, SqlStatement, TransformDefinition, TransformGroup, TransformGroupSetting, TransformKind,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

// [ISO/IEC 9075-2:2023 §11.63 to §11.71, §19.13] The words of these
// Formats that are no key words of SQL-92.

/// ASSIGNMENT, of AS ASSIGNMENT.
const ASSIGNMENT: &str = "ASSIGNMENT";
/// ORDERING, of CREATE and DROP ORDERING.
const ORDERING: &str = "ORDERING";
/// EQUALS, of EQUALS ONLY.
const EQUALS: &str = "EQUALS";
/// MAP, of a `<map category>`.
const MAP: &str = "MAP";
/// STATE, of a `<state category>`.
const STATE: &str = "STATE";
/// TRANSFORM, of the transform statements and SET TRANSFORM GROUP.
const TRANSFORM: &str = "TRANSFORM";
/// TRANSFORMS, of the transform statements.
const TRANSFORMS: &str = "TRANSFORMS";
/// TYPE, of SET TRANSFORM GROUP FOR TYPE.
const TYPE: &str = "TYPE";

impl Parser {
    /// Whether CREATE of a method, a cast, an ordering or a transform is
    /// next.
    pub(in crate::parser) fn next_is_of_type(&self) -> bool {
        let defined = self.next_is_cast() || self.next_is_ordering() || self.next_is_transform();
        self.next_is_method() || (self.is_kw(kw::CREATE) && defined)
    }

    /// The definition [ISO/IEC 9075-2:2023 §11.51] or alteration [§11.53]
    /// of a user-defined type, the definition of a method [§11.60], a cast
    /// [§11.63], an ordering [§11.65] or a transform [§11.67], or the
    /// alteration of a transform [§11.68], if one is next.
    pub(in crate::parser) fn defined_for_type(&mut self) -> Result<Option<SchemaStatement>> {
        let create = self.is_kw(kw::CREATE);
        Ok(Some(if self.next_is_type() && self.is_kw(kw::ALTER) {
            SchemaStatement::AlterType(self.alter_type()?)
        } else if self.next_is_type() {
            SchemaStatement::Type(self.type_definition()?)
        } else if self.next_is_method() {
            SchemaStatement::Method(Box::new(self.method_definition()?))
        } else if create && self.next_is_cast() {
            SchemaStatement::Cast(Box::new(self.cast_definition()?))
        } else if create && self.next_is_ordering() {
            SchemaStatement::Ordering(self.ordering_definition()?)
        } else if create && self.next_is_transform() {
            SchemaStatement::Transform(self.transform_definition()?)
        } else if self.is_kw(kw::ALTER) && self.next_is_transform() {
            SchemaStatement::AlterTransform(self.alter_transform()?)
        } else {
            return Ok(None);
        }))
    }

    /// Whether CREATE or DROP CAST is next.
    pub(in crate::parser) fn next_is_cast(&self) -> bool {
        self.next_is_kw(kw::CAST)
    }

    /// Whether CREATE or DROP ORDERING is next.
    pub(in crate::parser) fn next_is_ordering(&self) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == ORDERING)
    }

    /// Whether CREATE, ALTER or DROP TRANSFORM or TRANSFORMS is next.
    pub(in crate::parser) fn next_is_transform(&self) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == TRANSFORM || w == TRANSFORMS)
    }

    /// [ISO/IEC 9075-2:2023 §11.63] `<user-defined cast definition>` ::=
    /// CREATE CAST ( `<source data type>` AS `<target data type>` ) WITH
    /// `<cast function>` \[AS ASSIGNMENT\].
    pub(in crate::parser) fn cast_definition(&mut self) -> Result<CastDefinition> {
        let at = self.here();
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::CAST)?;
        let (source, target) = self.cast_types()?;
        self.expect_kw(kw::WITH)?;
        let function = self.routine_designator()?;
        let assignment = self.accept_kw(kw::AS);
        if assignment {
            self.expect_word(ASSIGNMENT)?;
        }
        Ok(CastDefinition { source, target, function, assignment, at })
    }

    /// ( `<source data type>` AS `<target data type>` ).
    fn cast_types(&mut self) -> Result<(crate::ast::TypeName, crate::ast::TypeName)> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let source = self.type_name()?;
        self.expect_kw(kw::AS)?;
        let target = self.type_name()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok((source, target))
    }

    /// What a DROP of a cast [ISO/IEC 9075-2:2023 §11.64], an ordering
    /// [§11.66] or a transform [§11.71] names, after DROP, its `<drop
    /// behavior>` next, if one is: CAST ( `<source data type>` AS `<target
    /// data type>` ); ORDERING FOR `<schema-resolved user-defined type
    /// name>`; {TRANSFORM | TRANSFORMS} {ALL | `<group name>`} FOR
    /// `<schema-resolved user-defined type name>`, none for ALL.
    pub(in crate::parser) fn dropped_of_type(&mut self) -> Result<Option<DropObject>> {
        let at = self.here();
        if self.accept_kw(kw::CAST) {
            return Ok(Some(DropObject::Cast(Box::new(self.cast_types()?), at)));
        }
        if self.accept_word(ORDERING) {
            self.expect_kw(kw::FOR)?;
            return Ok(Some(DropObject::Ordering(self.table_name()?)));
        }
        if !(self.accept_word(TRANSFORM) || self.accept_word(TRANSFORMS)) {
            return Ok(None);
        }
        let group = if self.accept_kw(kw::ALL) { None } else { Some(self.identifier()?) };
        self.expect_kw(kw::FOR)?;
        Ok(Some(DropObject::Transform(self.table_name()?, group)))
    }

    /// [ISO/IEC 9075-2:2023 §11.65] `<user-defined ordering definition>`
    /// ::= CREATE ORDERING FOR `<schema-resolved user-defined type name>`
    /// {EQUALS ONLY | ORDER FULL} BY `<ordering category>`.
    pub(in crate::parser) fn ordering_definition(&mut self) -> Result<OrderingDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.expect_word(ORDERING)?;
        self.expect_kw(kw::FOR)?;
        let udt = self.table_name()?;
        let form = if self.accept_word(EQUALS) {
            self.expect_kw(kw::ONLY)?;
            OrderingForm::Equals
        } else {
            self.expect_kw(kw::ORDER)?;
            self.expect_kw(kw::FULL)?;
            OrderingForm::Full
        };
        self.expect_kw(kw::BY)?;
        let category = if self.accept_kw(kw::RELATIVE) {
            self.expect_kw(kw::WITH)?;
            OrderingCategory::Relative(self.routine_designator()?)
        } else if self.accept_word(MAP) {
            self.expect_kw(kw::WITH)?;
            OrderingCategory::Map(self.routine_designator()?)
        } else {
            self.expect_word(STATE)?;
            // STATE [<specific name>]
            let named = matches!(self.peek(), Tok::Identifier(_));
            OrderingCategory::State(if named { Some(self.table_name()?) } else { None })
        };
        Ok(OrderingDefinition { udt, form, category })
    }

    /// [ISO/IEC 9075-2:2023 §11.67] `<transform definition>` ::= CREATE
    /// {TRANSFORM | TRANSFORMS} FOR `<schema-resolved user-defined type
    /// name>` `<transform group>`..., no two groups of one name \[SR2\].
    pub(in crate::parser) fn transform_definition(&mut self) -> Result<TransformDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.advance();
        self.expect_kw(kw::FOR)?;
        let udt = self.table_name()?;
        let mut groups: Vec<TransformGroup> = Vec::new();
        loop {
            let at = self.here();
            let name = (self.identifier()?, at);
            if groups.iter().any(|g| g.name.0 == name.0) {
                return Err(broken("§11.67 SR2", at, &format!("two <transform group>s are named {}", name.0)));
            }
            self.expect_delim(delim::LEFT_PAREN)?;
            let elements = self.transform_elements("§11.67 SR4b")?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            groups.push(TransformGroup { name, elements });
            if !matches!(self.peek(), Tok::Identifier(_)) {
                break;
            }
        }
        Ok(TransformDefinition { udt, groups })
    }

    /// `<transform element list>` ::= `<transform element>` \[, `<transform
    /// element>`\], `<transform element>` ::= TO SQL WITH `<to sql
    /// function>` | FROM SQL WITH `<from sql function>`, each kind at most
    /// once, as `rule` says.
    fn transform_elements(&mut self, rule: &str) -> Result<Vec<(TransformKind, RoutineDesignator)>> {
        let mut elements: Vec<(TransformKind, RoutineDesignator)> = Vec::new();
        loop {
            let at = self.here();
            let kind = self.transform_kind()?;
            if elements.iter().any(|(k, _)| *k == kind) {
                return Err(broken(rule, at, "a <transform element list> contains one <transform element> twice"));
            }
            self.expect_kw(kw::WITH)?;
            elements.push((kind, self.routine_designator()?));
            if !self.accept_delim(delim::COMMA) {
                return Ok(elements);
            }
        }
    }

    /// `<transform kind>` ::= TO SQL | FROM SQL.
    fn transform_kind(&mut self) -> Result<TransformKind> {
        let kind = if self.accept_kw(kw::TO) {
            TransformKind::ToSql
        } else {
            self.expect_kw(kw::FROM)?;
            TransformKind::FromSql
        };
        self.expect_kw(kw::SQL)?;
        Ok(kind)
    }

    /// [ISO/IEC 9075-2:2023 §11.68] `<alter transform statement>` ::=
    /// ALTER {TRANSFORM | TRANSFORMS} FOR `<schema-resolved user-defined
    /// type name>` `<alter group>`..., `<alter group>` ::= `<group name>` (
    /// `<alter transform action list>` ): one `AlterGroup` for each
    /// action, in order.
    pub(in crate::parser) fn alter_transform(&mut self) -> Result<AlterTransform> {
        self.expect_kw(kw::ALTER)?;
        self.advance();
        self.expect_kw(kw::FOR)?;
        let udt = self.table_name()?;
        let mut groups = Vec::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            self.expect_delim(delim::LEFT_PAREN)?;
            loop {
                groups.push(AlterGroup { name: (name.clone(), at), action: self.alter_transform_action()? });
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            if !matches!(self.peek(), Tok::Identifier(_)) {
                return Ok(AlterTransform { udt, groups });
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.69, §11.70] `<alter transform action>` ::=
    /// ADD ( `<transform element list>` ) | DROP ( `<transform kind>` \[,
    /// `<transform kind>`\] `<drop behavior>` ), each kind at most once
    /// [§11.69 SR4, §11.70 SR3].
    fn alter_transform_action(&mut self) -> Result<AlterGroupAction> {
        if self.accept_kw(kw::ADD) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let elements = self.transform_elements("§11.69 SR4")?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(AlterGroupAction::Add(elements));
        }
        self.expect_kw(kw::DROP)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut kinds = vec![self.transform_kind()?];
        if self.accept_delim(delim::COMMA) {
            let at = self.here();
            let kind = self.transform_kind()?;
            if kinds.contains(&kind) {
                return Err(broken("§11.70 SR3", at, "a <drop transform element list> names one <transform kind> twice"));
            }
            kinds.push(kind);
        }
        let behavior = self.drop_behavior()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(AlterGroupAction::Drop(kinds, behavior))
    }

    /// Whether a `<set transform group statement>` begins here: SET
    /// DEFAULT TRANSFORM GROUP, or SET TRANSFORM GROUP.
    pub(in crate::parser) fn at_set_transform_group(&self) -> bool {
        let transform = |n: usize| matches!(self.peek_at(n), Tok::Identifier(w) if w == TRANSFORM);
        let group = |n: usize| matches!(self.peek_at(n), Tok::KeyWord(kw::GROUP));
        (self.next_is_kw(kw::DEFAULT) && transform(2) && group(3)) || (transform(1) && group(2))
    }

    /// [ISO/IEC 9075-2:2023 §19.13] `<set transform group statement>` ::=
    /// SET {DEFAULT TRANSFORM GROUP `<value specification>` | TRANSFORM
    /// GROUP FOR TYPE `<path-resolved user-defined type name>` `<value
    /// specification>`}, the value a `<character string literal>` or, as
    /// \[EXTENSION\], the `<group name>` itself.
    pub(in crate::parser) fn set_transform_group(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::SET)?;
        let default = self.accept_kw(kw::DEFAULT);
        self.expect_word(TRANSFORM)?;
        self.expect_kw(kw::GROUP)?;
        let setting = if default {
            TransformGroupSetting::Default(self.group_value()?)
        } else {
            self.expect_kw(kw::FOR)?;
            self.expect_word(TYPE)?;
            let udt = self.table_name()?;
            TransformGroupSetting::ForType(udt, self.group_value()?)
        };
        Ok(SqlStatement::SetTransformGroup(setting))
    }

    /// The group named by a `<set transform group statement>`.
    fn group_value(&mut self) -> Result<String> {
        if let Tok::CharacterString(group) = self.peek().clone() {
            self.advance();
            return Ok(group);
        }
        self.identifier()
    }
}
