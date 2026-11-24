//! [ISO/IEC 9075-2:2023 §11.60, §11.62, §10.6, §16.1, §16.2] SQL-invoked
//! routines (Feature T321): `<SQL-invoked routine>` of an SQL routine
//! body, `<drop routine statement>`, `<call statement>` and `<return
//! statement>`. FUNCTION, RETURNS, CALL, RETURN and the other words of
//! these Formats are no key words of SQL-92, so each is read as an
//! identifier, a key word only here. The `<routine characteristics>` are
//! read in `routines/characteristics.rs`, CALL and RETURN in
//! `routines/call.rs`.
//!
//! While an `<SQL routine body>` is read the parser knows it is in one
//! (`RoutineScope`). Its statement then ends where its Format does, no
//! host or dynamic parameter may be written \[SR19\], and a `<target
//! specification>` is an `<SQL parameter reference>`.

mod call;
mod characteristics;
mod designator;

pub(super) use characteristics::Access;
pub(super) use designator::{CONSTRUCTOR, INSTANCE, METHOD};

use super::Parser;
use super::ptf::{ReturnsTable, has_table_parameter};
use crate::ast::{
    ParameterDeclaration, ParameterDefault, ParameterMode, ParameterSpecification, ParameterType, RoutineBody, RoutineDefinition, RoutineKind, SqlArgument,
    SqlStatement, TargetKind, TypeName,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 9075-2:2023 §11.60, §10.6, §10.4, §16.1, §16.2] The words of
// the Formats that are no key words of SQL-92.

/// FUNCTION: no SQL-92 key word, so a word [ISO/IEC 9075-2:2023 §11.60].
pub(super) const FUNCTION: &str = "FUNCTION";
/// ROUTINE: no SQL-92 key word, so a word [ISO/IEC 9075-2:2023 §10.6].
pub(super) const ROUTINE: &str = "ROUTINE";
/// SPECIFIC: no SQL-92 key word, so a word [ISO/IEC 9075-2:2023 §10.6].
pub(super) const SPECIFIC: &str = "SPECIFIC";
/// CALL: no SQL-92 key word, so a word [ISO/IEC 9075-2:2023 §16.1].
pub(super) const CALL: &str = "CALL";
/// RETURN: no SQL-92 key word, so a word [ISO/IEC 9075-2:2023 §16.2].
pub(super) const RETURN: &str = "RETURN";
/// RETURNS, of a `<returns clause>` and of RETURNS NULL ON NULL INPUT.
pub(super) const RETURNS: &str = "RETURNS";
/// OUT, a `<parameter mode>`.
pub(super) const OUT: &str = "OUT";
/// INOUT, a `<parameter mode>`.
pub(super) const INOUT: &str = "INOUT";
/// RESULT, after a `<parameter type>` and of DYNAMIC RESULT SETS.
pub(super) const RESULT: &str = "RESULT";
/// LOCATOR, of a `<locator indication>`.
const LOCATOR: &str = "LOCATOR";
/// STATIC, of a `<dispatch clause>`.
pub(super) const STATIC: &str = "STATIC";
/// DISPATCH, of a `<dispatch clause>`.
const DISPATCH: &str = "DISPATCH";
/// SECURITY, of a `<rights clause>`.
const SECURITY: &str = "SECURITY";
/// INVOKER, of a `<rights clause>`.
const INVOKER: &str = "INVOKER";
/// DEFINER, of a `<rights clause>`.
const DEFINER: &str = "DEFINER";
/// PRIVATE, which begins `<PTF private parameters>`.
pub(super) const PRIVATE: &str = "PRIVATE";
/// START, of START WITH in a `<polymorphic table function body>`.
pub(super) const START: &str = "START";
/// FULFILL, of FULFILL WITH in a `<polymorphic table function body>`.
pub(super) const FULFILL: &str = "FULFILL";
/// COPARTITION, which begins a `<co-partition clause>` [§10.4].
pub(super) const COPARTITION: &str = "COPARTITION";

/// [ISO/IEC 9075-2:2023 §11.60] What the reading of an `<SQL routine
/// body>` knows of its routine.
pub(super) struct RoutineScope {
    /// A function or a procedure: RETURN is in a function's body alone
    /// [§16.2 SR1].
    pub(super) kind: RoutineKind,
    /// The `<qualified identifier>` of its `<schema qualified routine
    /// name>`, which qualifies a reference to a parameter [§6.6 SR9a].
    pub(super) name: String,
    /// The names of its SQL parameters, in order.
    pub(super) parameters: Vec<String>,
    /// Whether it is no routine's but an SQL control statement's run
    /// directly, read as a procedure's body of no parameters.
    pub(super) direct: bool,
}

/// [ISO/IEC 9075-2:2023 §11.60 SR19] The refusal of a parameter or
/// variable in an `<SQL-invoked routine>`, at `at`.
fn parameter_refused(at: usize) -> SqlError {
    SqlError::at(
        Code::SyntaxRule,
        at,
        "[ISO/IEC 9075-2:2023 §11.60 SR19] an <SQL-invoked routine> shall not contain a <host parameter name>, a <dynamic \
         parameter specification> or an <embedded variable name>",
    )
}

/// The refusal at `at` of valid SQL turborel does not support, `msg`
/// citing the rule that names it.
pub(super) fn not_supported(at: usize, msg: &str) -> SqlError {
    SqlError::at(Code::Unsupported, at, msg)
}

/// The refusal of `what`, a cursor statement at `at` whose `<cursor name>`
/// breaks `rule`, since no cursor is declared in the scope of an `<SQL
/// routine body>`.
pub(super) fn cursor_refused(rule: &str, what: &str, at: usize) -> SqlError {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 {rule}] the <cursor name> of {what} shall be contained within the scope of a declared <cursor name>, \
         and an <SQL routine body> declares none"
    );
    SqlError::at(Code::SyntaxRule, at, msg)
}

/// Whether `tok` begins a `<dynamic parameter specification>` or an
/// `<embedded variable name>`.
fn is_parameter_token(tok: &Tok) -> bool {
    *tok == Tok::Delimiter(delim::QUESTION) || *tok == Tok::Delimiter(delim::COLON)
}

/// Whether `tok` ends an `<SQL parameter declaration>` whose `<parameter
/// type>` is the one token before it: a comma, a right parenthesis, or
/// what may follow a `<data type>` there.
fn ends_declaration(tok: &Tok) -> bool {
    let word = matches!(tok, Tok::Identifier(w) if w == RESULT);
    let key_word = *tok == Tok::KeyWord(kw::AS) || *tok == Tok::KeyWord(kw::DEFAULT);
    *tok == Tok::Delimiter(delim::COMMA) || *tok == Tok::Delimiter(delim::RIGHT_PAREN) || key_word || word
}

/// The refusal at `at` of a `<parameter default>` of a parameter of
/// `mode` of a `kind`, if one is refused: of a function's, not supported;
/// of an output parameter, a Syntax Rule's.
fn default_refusal(kind: RoutineKind, mode: Option<ParameterMode>, at: usize) -> Option<SqlError> {
    if kind == RoutineKind::Function {
        let msg = "[ISO/IEC 9075-2:2023 §11.60 CR34] a <parameter default> of a function needs Feature T525, “Default values for \
                   parameters of SQL-invoked functions”, which turborel does not support";
        return Some(not_supported(at, msg));
    }
    (mode == Some(ParameterMode::Out))
        .then(|| SqlError::at(Code::SyntaxRule, at, "[ISO/IEC 9075-2:2023 §11.60 SR16e ii] an output SQL parameter has no <parameter default>"))
}

impl Parser {
    /// Whether an `<SQL-invoked routine>` begins here: CREATE, then
    /// FUNCTION or PROCEDURE.
    pub(super) fn next_is_routine(&self) -> bool {
        let routine = self.next_is_kw(kw::PROCEDURE) || matches!(self.peek_at(1), Tok::Identifier(w) if w == FUNCTION);
        (self.is_kw(kw::CREATE) && routine) || self.next_is_distance_function()
    }

    /// [ISO/IEC 9075-2:2023 §11.60] `<SQL-invoked routine>` of an SQL
    /// routine body, from CREATE.
    ///
    /// # Errors
    ///
    /// The text is not one, or breaks a Syntax Rule.
    pub(super) fn routine_definition(&mut self) -> Result<RoutineDefinition> {
        let start = self.here();
        self.expect_kw(kw::CREATE)?;
        self.invoked_routine(start)
    }

    /// `<SQL-invoked procedure>` or `<SQL-invoked function>`, from
    /// PROCEDURE or FUNCTION, its text from `start`.
    pub(super) fn invoked_routine(&mut self, start: usize) -> Result<RoutineDefinition> {
        if self.is_word(super::vectors::VECTOR) {
            return self.distance_function(start);
        }
        let kind = if self.accept_kw(kw::PROCEDURE) {
            RoutineKind::Procedure
        } else {
            self.expect_word(FUNCTION)?;
            RoutineKind::Function
        };
        let name = self.table_name()?;
        // The first refusal of what an external routine alone may have,
        // given once the <language clause> shows the routine an SQL one.
        let mut sql_only = None;
        let parameters = self.parameter_declaration_list(kind, &mut sql_only)?;
        let (returns, table) = if kind == RoutineKind::Function { self.returns_clause(&mut sql_only)? } else { (None, None) };
        let table = self.polymorphic(&parameters, table)?;
        let traits = self.routine_characteristics(kind, sql_only, table.as_ref().map(|_| has_table_parameter(&parameters)))?;
        let body_start = self.here();
        let body = if let Some(table) = table {
            RoutineBody::Polymorphic(Box::new(self.polymorphic_body(table)?))
        } else {
            self.routine_spec(kind)?;
            let scope = RoutineScope {
                kind,
                name: name.ident.clone(),
                parameters: parameters.iter().map(|p| p.name.clone()).collect(),
                direct: false,
            };
            RoutineBody::Sql(Box::new(self.routine_body(scope)?))
        };
        let body_text = self.text_from(body_start);
        Ok(RoutineDefinition {
            kind,
            name,
            parameters,
            returns,
            specific: traits.specific,
            deterministic: traits.deterministic,
            access: traits.access,
            null_call: traits.null_call,
            new_savepoint_level: traits.new_savepoint_level,
            body,
            text: self.text_from(start),
            body_text,
            distance: None,
        })
    }

    /// `<SQL parameter declaration list>` ::= ( \[`<SQL parameter
    /// declaration>` \[{, ...}...\]\] ), of a `kind`, no two names
    /// equivalent \[SR15\].
    pub(super) fn parameter_declaration_list(
        &mut self,
        kind: RoutineKind,
        sql_only: &mut Option<SqlError>,
    ) -> Result<Vec<ParameterDeclaration>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut parameters: Vec<ParameterDeclaration> = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(parameters);
        }
        loop {
            let parameter = self.parameter_declaration(kind, sql_only)?;
            // An unnamed parameter, refused by SR14, has the empty name.
            let named = !parameter.name.is_empty();
            if named && parameters.iter().any(|p| p.name == parameter.name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR15] SQL parameter {} is declared twice", parameter.name);
                return Err(SqlError::at(Code::SyntaxRule, parameter.at, msg));
            }
            parameters.push(parameter);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(parameters)
    }

    /// `<SQL parameter declaration>` ::= \[`<parameter mode>`\] \[`<SQL
    /// parameter name>`\] `<parameter type>` \[RESULT\] \[DEFAULT
    /// `<parameter default>`\], of a `kind`.
    fn parameter_declaration(&mut self, kind: RoutineKind, sql_only: &mut Option<SqlError>) -> Result<ParameterDeclaration> {
        let at = self.here();
        let mode = self.parameter_mode();
        if mode.is_some() && kind == RoutineKind::Function {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                "[ISO/IEC 9075-2:2023 §11.60 SR13] a parameter of a function has no <parameter mode>",
            ));
        }
        let name = if self.at_unnamed_parameter() {
            sql_only.get_or_insert_with(|| self.rule("[ISO/IEC 9075-2:2023 §11.60 SR14] every SQL parameter of an SQL routine is named"));
            String::new()
        } else {
            self.identifier()?
        };
        let data_type = self.parameter_type(sql_only)?;
        if self.is_word(RESULT) {
            let msg = "[ISO/IEC 9075-2:2023 §11.60 CR5] RESULT needs Feature S024, “Enhanced structured types”, which turborel does not \
                       support";
            self.refuse_later(not_supported(self.here(), msg));
            self.advance();
        }
        let default = if self.is_kw(kw::DEFAULT) { self.parameter_default(kind, mode, &data_type)? } else { None };
        Ok(ParameterDeclaration { mode, name, data_type, default, at })
    }

    /// `<parameter mode>` ::= IN | OUT | INOUT, taken if next.
    pub(super) fn parameter_mode(&mut self) -> Option<ParameterMode> {
        if self.accept_kw(kw::IN) {
            Some(ParameterMode::In)
        } else if self.accept_word(OUT) {
            Some(ParameterMode::Out)
        } else if self.accept_word(INOUT) {
            Some(ParameterMode::InOut)
        } else {
            None
        }
    }

    /// Whether the `<parameter type>` is next, no `<SQL parameter name>`
    /// written: a key word, which no name is, a row type, or an identifier
    /// the declaration ends after.
    pub(super) fn at_unnamed_parameter(&self) -> bool {
        match self.peek() {
            Tok::KeyWord(_) => true,
            Tok::Identifier(_) => self.at_row_type() || ends_declaration(self.peek_at(1)),
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

    /// `<parameter type>` ::= `<data type>` \[`<locator indication>`\] |
    /// `<generic table parameter type>` | `<descriptor parameter type>`.
    fn parameter_type(&mut self, sql_only: &mut Option<SqlError>) -> Result<ParameterType> {
        if self.at_row_type() {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §11.60 SR16a] the <parameter type> of an SQL parameter shall not specify ROW"));
        }
        if let Some(generic) = self.generic_parameter_type()? {
            return Ok(generic);
        }
        let data_type = self.type_name()?;
        self.locator_indication("SR16b i", sql_only)?;
        Ok(ParameterType::Data(data_type))
    }

    /// \[`<locator indication>`\] ::= AS LOCATOR, which `rule` lets an
    /// external routine alone have.
    fn locator_indication(&mut self, rule: &str, sql_only: &mut Option<SqlError>) -> Result<()> {
        let at = self.here();
        if self.accept_kw(kw::AS) {
            self.expect_word(LOCATOR)?;
            let msg = format!("[ISO/IEC 9075-2:2023 §11.60 {rule}] a <locator indication> is of an external routine alone");
            sql_only.get_or_insert_with(|| SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok(())
    }

    /// DEFAULT `<parameter default>`, of a parameter of `mode` of a `kind`:
    /// a procedure's (Features T522 and T523); a function's read, then
    /// refused once the statement is; an output parameter's refused at
    /// once, as a Syntax Rule forbids it.
    ///
    /// Of a generic table parameter none is written [SR10g ii 1]; of a
    /// descriptor parameter it is a `<descriptor value constructor>`, which
    /// no other's is [SR10g iii]. As the routine's body, it contains no
    /// host or dynamic parameter [§11.60 SR19].
    fn parameter_default(&mut self, kind: RoutineKind, mode: Option<ParameterMode>, of: &ParameterType) -> Result<Option<ParameterDefault>> {
        let refused = default_refusal(kind, mode, self.here());
        if let Some(e) = refused.as_ref().filter(|e| e.code == Code::SyntaxRule) {
            return Err(e.clone());
        }
        if let ParameterType::Table(_) = of {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §11.60 SR10g ii 1] a generic table parameter has no <parameter default>"));
        }
        self.expect_kw(kw::DEFAULT)?;
        let descriptor = matches!(of, ParameterType::Descriptor);
        if descriptor != self.is_kw(kw::DESCRIPTOR) {
            return Err(self.rule(
                "[ISO/IEC 9075-2:2023 §11.60 SR10g iii] the <parameter default> of a descriptor parameter, and of no other, is a \
                 <descriptor value constructor>",
            ));
        }
        if descriptor {
            self.descriptor_value_constructor()?;
        }
        let start = self.here();
        let params = std::mem::take(&mut self.params);
        let dynamic = self.dynamic.take();
        let value = if descriptor { Ok(None) } else { self.default_value().map(Some) };
        (self.params, self.dynamic) = (params, dynamic);
        let value = value.map_err(|e| self.body_parameter_refusal(e))?;
        if let Some(e) = refused {
            self.refuse_later(e);
            return Ok(None);
        }
        Ok(value.map(|value| ParameterDefault { value, text: self.text_from(start) }))
    }

    /// `<parameter default>` ::= `<value expression>` | `<contextually
    /// typed value specification>`, of which turborel reads NULL, as the
    /// others are value expressions or no value of a parameter [§6.5].
    fn default_value(&mut self) -> Result<SqlArgument> {
        let at = self.here();
        if self.accept_kw(kw::NULL) {
            return Ok(SqlArgument::Null(at));
        }
        self.value_expression().map(SqlArgument::Value)
    }

    /// [ISO/IEC 9075-2:2023 §11.60] The `<parameter default>` a routine
    /// descriptor keeps, its text `parameter_default` read, as a CALL
    /// that takes it evaluates it [§9.18 GR3b ii 2].
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub fn kept_default(&mut self) -> Result<SqlArgument> {
        let value = self.default_value()?;
        self.end()?;
        Ok(value)
    }

    /// `<returns clause>` ::= RETURNS `<returns data type>`, of those
    /// implemented: no `<returns table type>`, and of an SQL routine no
    /// `<result cast>` \[SR9m i\].
    fn returns_clause(&mut self, sql_only: &mut Option<SqlError>) -> Result<(Option<TypeName>, Option<ReturnsTable>)> {
        self.expect_word(RETURNS)?;
        if self.at_returns_table() {
            return Ok((None, Some(self.returns_table_type()?)));
        }
        let data_type = self.type_name()?;
        self.locator_indication("SR9v i", sql_only)?;
        let at = self.here();
        if self.accept_kw(kw::CAST) {
            self.expect_kw(kw::FROM)?;
            self.type_name()?;
            self.locator_indication("SR9w i", sql_only)?;
            let msg = "[ISO/IEC 9075-2:2023 §11.60 SR9m i] the <returns clause> of an SQL routine has no <result cast>";
            sql_only.get_or_insert_with(|| SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok((Some(data_type), None))
    }

    /// What may stand between the `<routine characteristics>` of a `kind`
    /// and its `<SQL routine body>`: of those, a `<rights clause>` is read
    /// and refused once the statement is, and a `<dispatch clause>`, an
    /// `<external body reference>` and a `<polymorphic table function
    /// body>` are refused.
    pub(super) fn routine_spec(&mut self, kind: RoutineKind) -> Result<()> {
        let at = self.here();
        let dispatch = matches!(self.peek_at(1), Tok::Identifier(w) if w == DISPATCH);
        if kind == RoutineKind::Function && self.is_word(STATIC) && dispatch {
            let msg = "[ISO/IEC 9075-2:2023 §11.60 SR9r] a <dispatch clause>, of a function on a user-defined type, is valid SQL \
                       turborel does not support";
            return Err(not_supported(at, msg));
        }
        if self.accept_kw(kw::SQL) {
            self.expect_word(SECURITY)?;
            if !self.accept_word(INVOKER) {
                self.expect_word(DEFINER)?;
            }
            let msg = "[ISO/IEC 9075-2:2023 §11.60 CR24] a <rights clause> needs Feature T324, “Explicit security for SQL routines”, \
                       which turborel does not support";
            self.refuse_later(not_supported(at, msg));
        }
        if self.is_kw(kw::EXTERNAL) {
            return Err(
                self.rule("[ISO/IEC 9075-2:2023 §11.60 SR23a] an SQL routine has an <SQL routine spec>, not an <external body reference>")
            );
        }
        if self.at_ptf_body() {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                "[ISO/IEC 9075-2:2023 §11.60 SR7c] only a polymorphic table function, which returns a table, has a <polymorphic table \
                 function body>",
            ));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.60] `<SQL routine body>` ::= `<SQL
    /// procedure statement>`, read in `scope`, where no parameter is
    /// declared and none may be prepared \[SR19\], restoring both after.
    pub(super) fn routine_body(&mut self, scope: RoutineScope) -> Result<SqlStatement> {
        if self.is_kw(kw::CREATE) && self.next_is_kw(kw::SCHEMA) {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §11.60 SR23g] an <SQL routine body> shall not contain a <schema definition>"));
        }
        if let Some(refused) = self.cursor_refusal() {
            return Err(refused);
        }
        let params = std::mem::take(&mut self.params);
        let dynamic = self.dynamic.take();
        let blocks = std::mem::take(&mut self.blocks);
        let outer = self.routine.replace(scope);
        let body_start = self.here();
        let result = self.sql_statement();
        self.routine = outer;
        self.blocks = blocks;
        self.params = params;
        self.dynamic = dynamic;
        let body = result.map_err(|e| self.body_parameter_refusal(e))?;
        Self::dynamic_allowed(&body, body_start)?;
        if let SqlStatement::UpdatePositioned { cursor: (_, cursor), .. } = &body {
            return Err(cursor_refused("§14.13 SR1", "an <update statement: positioned>", *cursor));
        }
        if let SqlStatement::DeletePositioned { cursor: (_, cursor), .. } = &body {
            return Err(cursor_refused("§14.8 SR1", "a <delete statement: positioned>", *cursor));
        }
        Ok(body)
    }

    /// The refusal of the OPEN, FETCH or CLOSE next, if one is: no cursor
    /// is declared in the scope of an `<SQL routine body>`.
    pub(super) fn cursor_refusal(&self) -> Option<SqlError> {
        let at = self.here();
        if self.is_kw(kw::OPEN) {
            return Some(cursor_refused("§14.4 SR1", "an <open statement>", at));
        }
        // [ISO/IEC 9075-2:2023 §20.12] a <dynamic fetch statement> of a
        // <PTF cursor name> is of a PTF component procedure's body.
        if self.is_kw(kw::FETCH) && !self.at_ptf_fetch() {
            return Some(cursor_refused("§14.5 SR3", "a <fetch statement>", at));
        }
        self.is_kw(kw::CLOSE).then(|| cursor_refused("§14.6 SR1", "a <close statement>", at))
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR19] `e`, or the refusal of SR19 if
    /// `e` is the refusal of a `<dynamic parameter specification>` or an
    /// `<embedded variable name>`, which the statement parsers give where
    /// one is written.
    fn body_parameter_refusal(&self, e: SqlError) -> SqlError {
        let Some(at) = e.pos else { return e };
        if self.toks.iter().any(|t| t.start == at && is_parameter_token(&t.tok)) { parameter_refused(at) } else { e }
    }

    /// The `<target specification>` of a `<triggered action>` being read
    /// (`transition_target`), or of an `<SQL routine body>`
    /// (`routine_target`).
    pub(super) fn scoped_target(&mut self) -> Result<ParameterSpecification> {
        // [ISO/IEC 9075-4:2006 draft §6.2] an SQL variable's scope is
        // within the routine's or the trigger's.
        if let Some(variable) = self.variable_target() {
            return Ok(variable);
        }
        if self.trigger.is_some() { self.transition_target() } else { self.routine_target() }
    }

    /// [ISO/IEC 9075-2:2023 §6.4, §6.6 SR16] A `<target specification>` or
    /// `<simple target specification>` in an `<SQL routine body>`, where
    /// the one kind it may be is an `<SQL parameter reference>`: `P` or
    /// `R`.`P` of a parameter `P` of the routine `R`, marked as such.
    ///
    /// # Errors
    ///
    /// The text is not one: a column reference, which no target in a
    /// routine may be [§6.4 SR7], or a parameter or variable \[§11.60
    /// SR19\].
    pub(super) fn routine_target(&mut self) -> Result<ParameterSpecification> {
        let at = self.here();
        if is_parameter_token(self.peek()) {
            return Err(parameter_refused(at));
        }
        if let Some((name, qualified)) = self.parameter_reference() {
            self.advance();
            if qualified {
                self.advance();
                self.advance();
            }
            return Ok(ParameterSpecification::of(name, TargetKind::Routine, at));
        }
        if let Tok::Identifier(name) = self.peek() {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §6.4 SR7] {name} is no SQL parameter of the routine, and a <target specification> that is a \
                 <column reference> shall be a new transition variable column reference"
            );
            return Err(self.rule(msg));
        }
        Err(self.syntax("expected a <target specification>"))
    }

    /// The `<SQL parameter reference>` next, `P` or `R`.`P` of a parameter
    /// P of the routine R whose body is read, and whether it is the
    /// latter; none if none is next.
    pub(super) fn parameter_reference(&self) -> Option<(String, bool)> {
        let scope = self.routine.as_ref()?;
        let Tok::Identifier(first) = self.peek() else { return None };
        let period = *self.peek_at(1) == Tok::Delimiter(delim::PERIOD);
        let qualified = period && *first == scope.name;
        let named = if qualified { self.peek_at(2) } else { self.peek() };
        let Tok::Identifier(parameter) = named else { return None };
        // [ISO/IEC 9075-4:2006 draft §14.5] P.F, a field of a row P holds,
        // as an assignment's target.
        let whole = qualified || !period || (!self.blocks.is_empty() && scope.parameters.contains(first));
        (whole && scope.parameters.contains(parameter)).then(|| (parameter.clone(), qualified))
    }
}

// Unit tests: each form of the statements parsed, and each Syntax Rule
// refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
