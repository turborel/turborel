//! \[EXTENSION\] The definition and drop of a distance function of VECTOR
//! (design document §8, RULES.md D93): an SQL-invoked function of two
//! vectors whose body is a `<value expression>` of them, which
//! VECTOR_DISTANCE and a vector index then name as a metric.
//!
//! ```text
//! <distance function definition> ::=
//!     CREATE VECTOR DISTANCE FUNCTION <schema qualified routine name>
//!     ( <SQL parameter name> VECTOR , <SQL parameter name> VECTOR )
//!     RETURNS { FLOAT64 | <data type> }
//!     [ DETERMINISTIC ]
//!     [ PROPERTIES ( <distance property> [ { , <distance property> }... ] ) ]
//!     [ VALID FORMATS ( <format> [ { , <format> }... ] ) ]
//!     LANGUAGE SQL
//!     AS <character string literal>
//! <distance property> ::= SYMMETRIC | NON_NEGATIVE | TRIANGLE_INEQUALITY
//! <format> ::= FLOAT32 | FLOAT16 | FLOAT64 | INT8 | BINARY
//! <drop distance function statement> ::=
//!     DROP VECTOR DISTANCE FUNCTION <schema qualified routine name> <drop behavior>
//! <from vector> ::=
//!     FROM_VECTOR <left paren> <value expression>
//!     [ RETURNING { VARCHAR | <character string type> } ]
//!     [ FORMAT { DENSE | SPARSE } ] <right paren>
//! ```
//!
//! FROM_VECTOR returning a character large object is FROM_VECTOR_CLOB, and
//! its FORMAT the layout argument either takes.
//!
//! The literal is the body's `<value expression>`, its routine body
//! `RETURN` of it. The function is a null-call function that reads no
//! SQL data. Only LANGUAGE SQL is built in.

use super::Parser;
use super::routines::{FUNCTION, RETURN, RoutineScope};
use crate::ast::{
    DataAccess, Literal, ParameterType, RoutineBody, RoutineDefinition, RoutineDesignator, RoutineKind, RoutineType, TypeName,
    ValueExpression, ValueSpecification,
};
use crate::catalog::DistanceDesc;
use crate::error::{Code, Result, SqlError};
use crate::functions::FORMAT;
use crate::lexer::{Tok, delim, kw};
use crate::value::{DOUBLE_PRECISION, DataType, Spelled};
use crate::vector::{Element, Layout};

/// VECTOR, which begins the definition's kind.
pub(super) const VECTOR: &str = "VECTOR";
/// DISTANCE.
const DISTANCE: &str = "DISTANCE";
/// FLOAT64, the result type the design document writes.
const FLOAT64: &str = "FLOAT64";
/// DETERMINISTIC.
const DETERMINISTIC: &str = "DETERMINISTIC";
/// PROPERTIES.
const PROPERTIES: &str = "PROPERTIES";
/// VALID, of VALID FORMATS.
const VALID: &str = "VALID";
/// FORMATS.
const FORMATS: &str = "FORMATS";
/// The distance properties.
const SYMMETRIC: &str = "SYMMETRIC";
/// NON_NEGATIVE.
const NON_NEGATIVE: &str = "NON_NEGATIVE";
/// TRIANGLE_INEQUALITY.
const TRIANGLE_INEQUALITY: &str = "TRIANGLE_INEQUALITY";
/// FROM_VECTOR, whose argument RETURNING and FORMAT may follow.
pub(super) const FROM_VECTOR: &str = "FROM_VECTOR";
/// FROM_VECTOR returning a character large object.
const FROM_VECTOR_CLOB: &str = "FROM_VECTOR_CLOB";
/// RETURNING.
const RETURNING: &str = "RETURNING";
/// The parameters a distance function has.
const PARAMETERS: usize = 2;

/// Whether `t`, a parameter's type as written, is VECTOR.
fn is_vector(t: &ParameterType) -> bool {
    match t {
        ParameterType::Data(TypeName::Domain(name) | TypeName::Extension { name, .. }) => name.auth.is_none() && name.ident == VECTOR,
        ParameterType::Data(_) | ParameterType::Table(_) | ParameterType::Descriptor => false,
    }
}

impl Parser {
    /// Whether DROP VECTOR DISTANCE FUNCTION begins here, after DROP.
    pub(super) fn at_distance_function(&self) -> bool {
        let word = |n: usize, w: &str| matches!(self.peek_at(n), Tok::Identifier(x) if x == w);
        word(0, VECTOR) && word(1, DISTANCE)
    }

    /// Whether CREATE VECTOR DISTANCE begins here.
    pub(super) fn next_is_distance_function(&self) -> bool {
        let word = |n: usize, w: &str| matches!(self.peek_at(n), Tok::Identifier(x) if x == w);
        self.is_kw(kw::CREATE) && word(1, VECTOR) && word(2, DISTANCE)
    }

    /// `<drop distance function statement>` from VECTOR: the function's
    /// designator, as DROP FUNCTION names it.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn dropped_distance_function(&mut self) -> Result<RoutineDesignator> {
        self.expect_word(VECTOR)?;
        self.expect_word(DISTANCE)?;
        self.expect_word(FUNCTION)?;
        let name = self.table_name()?;
        Ok(RoutineDesignator { routine_type: RoutineType::Function, specific: false, name, parameters: None, udt: None })
    }

    /// `<distance function definition>` from VECTOR, its text from
    /// `start`: an SQL-invoked function of what it declares more.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if the text is not one; -103 if it has other than two
    /// VECTOR parameters, or names a property or format twice or none
    /// there is; -900 of another language than SQL.
    pub(super) fn distance_function(&mut self, start: usize) -> Result<RoutineDefinition> {
        self.expect_word(VECTOR)?;
        self.expect_word(DISTANCE)?;
        self.expect_word(FUNCTION)?;
        let name = self.table_name()?;
        let mut sql_only = None;
        let parameters = self.parameter_declaration_list(RoutineKind::Function, &mut sql_only)?;
        if let Some(refused) = sql_only {
            return Err(refused);
        }
        if parameters.len() != PARAMETERS || !parameters.iter().all(|p| is_vector(&p.data_type)) {
            return Err(self.rule("[EXTENSION] a distance function has two parameters, each VECTOR"));
        }
        self.expect_word(super::routines::RETURNS)?;
        let returns = if self.accept_word(FLOAT64) {
            TypeName::Data(DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision })
        } else {
            self.type_name()?
        };
        let deterministic = self.accept_word(DETERMINISTIC);
        let declared = self.distance_declared()?;
        let scope = RoutineScope {
            kind: RoutineKind::Function,
            name: name.ident.clone(),
            parameters: parameters.iter().map(|p| p.name.clone()).collect(),
            direct: false,
        };
        // The body, read as a function's RETURN of it.
        let body_text = format!("{RETURN} {}", declared.expression);
        let mut body = Parser::with_extensions(&body_text, self.extensions)?;
        let statement = body.routine_body(scope)?;
        body.end()?;
        Ok(RoutineDefinition {
            kind: RoutineKind::Function,
            name,
            parameters,
            returns: Some(returns),
            specific: None,
            deterministic,
            access: DataAccess::Contains,
            null_call: true,
            new_savepoint_level: false,
            body: RoutineBody::Sql(Box::new(statement)),
            text: self.text_from(start),
            body_text,
            distance: Some(Box::new(declared)),
        })
    }

    /// What a distance function declares after DETERMINISTIC: its
    /// properties, its formats, its language and its body.
    fn distance_declared(&mut self) -> Result<DistanceDesc> {
        let mut declared =
            DistanceDesc { symmetric: false, non_negative: false, triangle: false, formats: Vec::new(), expression: String::new() };
        if self.accept_word(PROPERTIES) {
            self.distance_properties(&mut declared)?;
        }
        if self.accept_word(VALID) {
            self.expect_word(FORMATS)?;
            self.distance_formats(&mut declared)?;
        }
        self.distance_language()?;
        self.expect_kw(kw::AS)?;
        declared.expression = self.character_string_literal("body of a distance function")?;
        Ok(declared)
    }

    /// `( <distance property> [, ...] )`, each once.
    fn distance_properties(&mut self, declared: &mut DistanceDesc) -> Result<()> {
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            let word = self.identifier()?;
            let slot = match word.as_str() {
                SYMMETRIC => &mut declared.symmetric,
                NON_NEGATIVE => &mut declared.non_negative,
                TRIANGLE_INEQUALITY => &mut declared.triangle,
                _ => {
                    return Err(
                        self.rule(format!("[EXTENSION] {word} is no distance property: SYMMETRIC, NON_NEGATIVE or TRIANGLE_INEQUALITY"))
                    );
                }
            };
            if *slot {
                return Err(self.rule(format!("[EXTENSION] the property {word} is written twice")));
            }
            *slot = true;
            if !self.accept_delim(delim::COMMA) {
                return self.expect_delim(delim::RIGHT_PAREN);
            }
        }
    }

    /// `( <format> [, ...] )`, each once.
    fn distance_formats(&mut self, declared: &mut DistanceDesc) -> Result<()> {
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            let word = self.identifier()?;
            let element = Element::named(&word).ok_or_else(|| self.rule(format!("[EXTENSION] {word} is no vector format")))?;
            if declared.formats.contains(&element.code()) {
                return Err(self.rule(format!("[EXTENSION] the format {word} is written twice")));
            }
            declared.formats.push(element.code());
            if !self.accept_delim(delim::COMMA) {
                return self.expect_delim(delim::RIGHT_PAREN);
            }
        }
    }

    /// LANGUAGE SQL: another language is valid in the design document,
    /// and no other is built in.
    fn distance_language(&mut self) -> Result<()> {
        self.expect_kw(kw::LANGUAGE)?;
        let at = self.here();
        if self.accept_kw(kw::SQL) {
            return Ok(());
        }
        let language = self.identifier()?;
        Err(SqlError::at(
            Code::Unsupported,
            at,
            format!("[EXTENSION] a distance function of LANGUAGE {language} is valid, and only LANGUAGE SQL is built in"),
        ))
    }

    /// The clauses of `<from vector>` after its argument: `name` made
    /// FROM_VECTOR_CLOB if it returns a character large object, and the
    /// layout FORMAT names added to `args`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it returns other than a character string, or FORMAT
    /// names no layout.
    pub(super) fn vector_text_clauses(&mut self, name: &mut String, args: &mut Vec<ValueExpression>) -> Result<()> {
        if self.accept_word(RETURNING) {
            let alone = self.is_kw(kw::VARCHAR) && !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
            let returns = if alone {
                self.advance();
                DataType::varying(1)
            } else {
                match self.type_name()? {
                    TypeName::Data(t) => t,
                    TypeName::Domain(_)
                    | TypeName::Reference(..)
                    | TypeName::Collection(..)
                    | TypeName::Row(_)
                    | TypeName::Extension { .. } => {
                        return Err(self.rule("[EXTENSION] FROM_VECTOR returns VARCHAR or CLOB"));
                    }
                }
            };
            if !returns.is_character() {
                return Err(self.rule(format!("[EXTENSION] FROM_VECTOR returns VARCHAR or CLOB, not {returns}")));
            }
            if returns.is_lob() {
                FROM_VECTOR_CLOB.clone_into(name);
            }
        }
        if self.accept_word(FORMAT) {
            let word = self.identifier()?;
            let layout = Layout::named(&word).ok_or_else(|| self.rule(format!("[EXTENSION] FORMAT is DENSE or SPARSE, not {word}")))?;
            args.push(ValueExpression::Value(ValueSpecification::Literal(Literal::Character(layout.name().to_owned()))));
        }
        Ok(())
    }
}
