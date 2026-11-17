//! [ISO/IEC 9075-2:2023 §11.53 to §11.58] The `<alter type statement>`,
//! and [§11.60] the `<SQL-invoked method>`: CREATE of a `<method
//! specification designator>` and its `<routine body>`, read as an SQL
//! routine's is (`routines.rs`).

use super::{Defined, SELF, broken};
use crate::ast::{
    AlterType, AlterTypeAction, MethodDefinition, MethodDesignator, MethodKind, MethodSignature, RoutineBody, RoutineKind, TableName,
};
use crate::error::Result;
use crate::lexer::{Tok, kw};
use crate::parser::Parser;
use crate::parser::routines::{CONSTRUCTOR, INSTANCE, METHOD, RESULT, RoutineScope, SPECIFIC, STATIC};
use crate::parser::user_types::TYPE;

/// ATTRIBUTE, of ADD ATTRIBUTE and DROP ATTRIBUTE [ISO/IEC 9075-2:2023
/// §11.54, §11.55].
const ATTRIBUTE: &str = "ATTRIBUTE";
/// RETURNS, of the `<returns clause>` of a `<method specification
/// designator>`.
const RETURNS: &str = "RETURNS";

/// Whether `tok` is the word `word`.
fn is(tok: &Tok, word: &str) -> bool {
    matches!(tok, Tok::Identifier(w) if w == word)
}

impl Parser {
    /// Whether an `<SQL-invoked method>` begins here: CREATE, then
    /// \[INSTANCE | STATIC | CONSTRUCTOR\] METHOD or SPECIFIC METHOD.
    pub(in crate::parser) fn next_is_method(&self) -> bool {
        let kind = [INSTANCE, STATIC, CONSTRUCTOR, SPECIFIC].iter().any(|w| is(self.peek_at(1), w));
        let method = if kind { self.peek_at(2) } else { self.peek_at(1) };
        self.is_kw(kw::CREATE) && is(method, METHOD)
    }

    /// [ISO/IEC 9075-2:2023 §11.53] `<alter type statement>` ::= ALTER
    /// TYPE `<schema-resolved user-defined type name>` `<alter type
    /// action>`.
    pub(in crate::parser) fn alter_type(&mut self) -> Result<AlterType> {
        self.expect_kw(kw::ALTER)?;
        self.expect_word(TYPE)?;
        let name = self.table_name()?;
        let action = if self.accept_kw(kw::ADD) {
            if self.accept_word(ATTRIBUTE) {
                AlterTypeAction::AddAttribute(self.attribute_definition()?)
            } else {
                AlterTypeAction::AddMethod(self.method_specification(&name, Defined::Altered)?)
            }
        } else {
            self.expect_kw(kw::DROP)?;
            let action = if self.accept_word(ATTRIBUTE) {
                let at = self.here();
                AlterTypeAction::DropAttribute(self.identifier()?, at)
            } else {
                AlterTypeAction::DropMethod(self.method_designator()?)
            };
            self.expect_kw(kw::RESTRICT)?;
            action
        };
        Ok(AlterType { name, action })
    }

    /// [ISO/IEC 9075-2:2023 §11.58] `<specific method specification
    /// designator>` ::= \[INSTANCE | STATIC | CONSTRUCTOR\] METHOD `<method
    /// name>` `<data type list>`.
    fn method_designator(&mut self) -> Result<MethodDesignator> {
        let kind = self.method_kind();
        self.expect_word(METHOD)?;
        let at = self.here();
        let name = (self.identifier()?, at);
        Ok(MethodDesignator { kind, name, parameters: self.data_type_list()? })
    }

    /// [ISO/IEC 9075-2:2023 §11.60] `<SQL-invoked method>` ::= CREATE
    /// `<method specification designator>` `<routine body>`, the body an
    /// SQL routine's [SR23a], read as a function's whose parameters are
    /// SELF, but of a static method, and those declared.
    pub(in crate::parser) fn method_definition(&mut self) -> Result<MethodDefinition> {
        let start = self.here();
        self.expect_kw(kw::CREATE)?;
        let signature = self.method_signature()?;
        let (name, parameters) = match &signature {
            MethodSignature::Specific(specific) => (specific.ident.clone(), vec![SELF.to_owned()]),
            MethodSignature::Signature { kind, name, parameters, .. } => {
                let declared = parameters.iter().map(|p| p.name.clone());
                let own = (*kind != MethodKind::Static).then(|| SELF.to_owned());
                (name.0.clone(), own.into_iter().chain(declared).collect())
            }
        };
        self.routine_spec(RoutineKind::Function)?;
        let body_start = self.here();
        let scope = RoutineScope { kind: RoutineKind::Function, name, parameters, direct: false };
        let body = RoutineBody::Sql(Box::new(self.routine_body(scope)?));
        let body_text = self.text_from(body_start);
        Ok(MethodDefinition { signature, body, text: self.text_from(start), body_text, at: start })
    }

    /// [ISO/IEC 9075-2:2023 §11.60] `<method specification designator>`
    /// ::= SPECIFIC METHOD `<specific method name>` | \[INSTANCE | STATIC |
    /// CONSTRUCTOR\] METHOD `<method name>` `<SQL parameter declaration
    /// list>` \[`<returns clause>`\] FOR `<schema-resolved user-defined type
    /// name>`; \[EXTENSION\] SELF AS RESULT may follow the `<returns
    /// clause>`, as the method's specification says it.
    fn method_signature(&mut self) -> Result<MethodSignature> {
        if self.accept_word(SPECIFIC) {
            self.expect_word(METHOD)?;
            return Ok(MethodSignature::Specific(self.table_name()?));
        }
        let kind = self.method_kind();
        self.expect_word(METHOD)?;
        let at = self.here();
        let name = (self.identifier()?, at);
        let parameters = self.method_parameters(Defined::Type, true)?;
        let (returns, self_as_result) = if self.accept_word(RETURNS) {
            let returns = self.type_name()?;
            (Some(returns), self.self_as_result()?)
        } else {
            (None, false)
        };
        self.expect_kw(kw::FOR)?;
        let udt: TableName = self.table_name()?;
        if kind == MethodKind::Static && self_as_result {
            return Err(broken("§11.51 SR9b ii 1", at, "a static method is not SELF AS RESULT"));
        }
        Ok(MethodSignature::Signature { kind, name, parameters, returns, self_as_result, udt })
    }

    /// SELF AS RESULT, read if next: whether it was.
    fn self_as_result(&mut self) -> Result<bool> {
        if !(self.is_word(SELF) && self.next_is_kw(kw::AS)) {
            return Ok(false);
        }
        self.advance();
        self.advance();
        self.expect_word(RESULT)?;
        Ok(true)
    }
}
