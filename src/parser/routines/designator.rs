//! [ISO/IEC 9075-2:2023 §10.6] The `<specific routine designator>` DROP,
//! GRANT, REVOKE and the definitions of casts, orderings and transforms
//! name a routine by: SPECIFIC and its `<specific name>`, or its name,
//! perhaps its `<data type list>`, and of a method its kind and FOR its
//! type. ROUTINE, FUNCTION, METHOD and the kinds of a method are no key
//! words of SQL-92, so each is read as an identifier.

use super::{FUNCTION, ROUTINE, SPECIFIC, STATIC};
use crate::ast::{MethodKind, RoutineDesignator, RoutineType, TableName};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// METHOD, of a `<routine type>` [ISO/IEC 9075-2:2023 §10.6] and of the
/// method specifications [§11.51].
pub(in crate::parser) const METHOD: &str = "METHOD";
/// INSTANCE, the kind of a method of SELF.
pub(in crate::parser) const INSTANCE: &str = "INSTANCE";
/// CONSTRUCTOR, the kind of a constructor method.
pub(in crate::parser) const CONSTRUCTOR: &str = "CONSTRUCTOR";

/// Whether `tok` begins a `<routine type>`.
fn begins_routine_type(tok: &Tok) -> bool {
    match tok {
        Tok::KeyWord(word) => *word == kw::PROCEDURE,
        Tok::Identifier(w) => w == FUNCTION || w == ROUTINE || w == METHOD || w == INSTANCE || w == STATIC || w == CONSTRUCTOR,
        Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => false,
    }
}

impl Parser {
    /// Whether a `<specific routine designator>` begins here (`object` of
    /// a GRANT, REVOKE or DROP): a `<routine type>`, perhaps after
    /// SPECIFIC; a kind of a method only before METHOD.
    pub(in crate::parser) fn next_is_routine_designator(&self) -> bool {
        let first = usize::from(self.is_word(SPECIFIC));
        let kind = matches!(self.peek_at(first), Tok::Identifier(w) if w == INSTANCE || w == STATIC || w == CONSTRUCTOR);
        let method = matches!(self.peek_at(first.saturating_add(1)), Tok::Identifier(w) if w == METHOD);
        begins_routine_type(self.peek_at(first)) && (!kind || method)
    }

    /// [ISO/IEC 9075-2:2023 §10.6] `<specific routine designator>` ::=
    /// SPECIFIC `<routine type>` `<specific name>` | `<routine type>`
    /// `<member name>` \[FOR `<schema-resolved user-defined type name>`\],
    /// the type named if and only if the `<routine type>` is METHOD
    /// \[SR3a\], whose `<method name>` is an identifier.
    ///
    /// # Errors
    ///
    /// The text is not one, or breaks SR3a.
    pub(in crate::parser) fn routine_designator(&mut self) -> Result<RoutineDesignator> {
        let specific = self.accept_word(SPECIFIC);
        let routine_type = self.routine_type()?;
        let method = matches!(routine_type, RoutineType::Method(_));
        if specific {
            let name = self.table_name()?;
            return Ok(RoutineDesignator { routine_type, specific, name, parameters: None, udt: None });
        }
        let name = if method {
            let at = self.here();
            TableName { auth: None, ident: self.identifier()?, at }
        } else {
            self.table_name()?
        };
        let parameters = if self.is_delim(delim::LEFT_PAREN) { Some(self.data_type_list()?) } else { None };
        if self.is_kw(kw::FOR) != method {
            return Err(self.rule(
                "[ISO/IEC 9075-2:2023 §10.6 SR3a] a <specific routine designator> names a <schema-resolved user-defined type name> \
                 if and only if its <routine type> is METHOD",
            ));
        }
        let udt = if self.accept_kw(kw::FOR) { Some(self.table_name()?) } else { None };
        Ok(RoutineDesignator { routine_type, specific, name, parameters, udt })
    }

    /// `<routine type>` ::= ROUTINE | FUNCTION | PROCEDURE | \[INSTANCE |
    /// STATIC | CONSTRUCTOR\] METHOD, INSTANCE if no kind is written
    /// \[SR2\].
    fn routine_type(&mut self) -> Result<RoutineType> {
        if self.accept_kw(kw::PROCEDURE) {
            return Ok(RoutineType::Procedure);
        }
        if self.accept_word(FUNCTION) {
            return Ok(RoutineType::Function);
        }
        if self.accept_word(ROUTINE) {
            return Ok(RoutineType::Routine);
        }
        let kind = self.method_kind();
        if self.accept_word(METHOD) {
            return Ok(RoutineType::Method(kind));
        }
        Err(self.syntax("expected ROUTINE, FUNCTION, PROCEDURE or METHOD"))
    }

    /// \[INSTANCE | STATIC | CONSTRUCTOR\], read if written: INSTANCE if
    /// none is [ISO/IEC 9075-2:2023 §10.6 SR2, §11.51 SR9b i].
    pub(in crate::parser) fn method_kind(&mut self) -> MethodKind {
        if self.accept_word(STATIC) {
            MethodKind::Static
        } else if self.accept_word(CONSTRUCTOR) {
            MethodKind::Constructor
        } else {
            self.accept_word(INSTANCE);
            MethodKind::Instance
        }
    }
}
