//! [ISO/IEC 9075-2:2023 §11.51, §11.52] The parts of a `<user-defined
//! type definition>` of a structured type: its `<member list>` of
//! `<attribute definition>`s and its `<method specification list>`, each
//! checked against the Syntax Rules that need no catalog. The other
//! statements of structured types are read in the submodules: ALTER TYPE
//! and CREATE METHOD (`altered.rs`), casts, orderings and transforms
//! (`defined.rs`), reference types, typed tables and views (`typed.rs`),
//! and the expressions of user-defined types and references
//! (`values.rs`). Their words are no key words of SQL-92, so each is read
//! as an identifier.

mod altered;
mod defined;
mod typed;
mod values;

pub(super) use values::SPECIFICTYPE;

use super::Parser;
use super::routines::{Access, METHOD, RESULT};
use crate::ast::{
    AttributeDefinition, DataAccess, MethodCharacteristics, MethodKind, MethodSpecification, ParameterDeclaration, ParameterType,
    TableName, TypeName,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 9075-2:2023 §11.51] The words of a <method specification>.

/// OVERRIDING, which begins an `<overriding method specification>`.
const OVERRIDING: &str = "OVERRIDING";
/// SELF, of SELF AS RESULT and SELF AS LOCATOR, and the name of the
/// implicit first parameter of an instance method \[SR9b viii 2\].
pub(super) const SELF: &str = "SELF";
/// RETURNS, of a `<returns clause>`.
const RETURNS: &str = "RETURNS";
/// LOCATOR, of SELF AS LOCATOR and of a `<locator indication>`.
const LOCATOR: &str = "LOCATOR";
/// PARAMETER, of a `<parameter style clause>`.
const PARAMETER: &str = "PARAMETER";
/// STYLE, of a `<parameter style clause>`.
const STYLE: &str = "STYLE";
/// GENERAL, a `<parameter style>`.
const GENERAL: &str = "GENERAL";
/// SPECIFIC, before a `<specific method name>`.
const SPECIFIC: &str = "SPECIFIC";
/// The words that begin a `<method specification>`.
pub(super) const METHOD_WORDS: [&str; 5] = ["METHOD", "INSTANCE", "STATIC", "CONSTRUCTOR", "OVERRIDING"];

/// Where a method specification is read, which names the rules that
/// check it: a `<user-defined type definition>` [ISO/IEC 9075-2:2023
/// §11.51] or an `<alter type statement>` [§11.56, §11.57].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Defined {
    /// CREATE TYPE.
    Type,
    /// ALTER TYPE ... ADD.
    Altered,
}

impl Defined {
    /// The rule of the pair `(of CREATE TYPE, of ALTER TYPE)` that holds here.
    const fn of(self, rules: (&'static str, &'static str)) -> &'static str {
        match self {
            Defined::Type => rules.0,
            Defined::Altered => rules.1,
        }
    }
}

/// The refusal at `at` under `rule` of ISO/IEC 9075-2:2023, which `msg`
/// states.
pub(super) fn broken(rule: &str, at: usize, msg: &str) -> SqlError {
    SqlError::at(Code::SyntaxRule, at, format!("[ISO/IEC 9075-2:2023 {rule}] {msg}"))
}

/// Whether `t` names the type `udt`: its name, of no schema or of the
/// same.
pub(super) fn names_type(t: &TypeName, udt: &TableName) -> bool {
    let TypeName::Domain(name) = t else { return false };
    name.ident == udt.ident && (name.auth.is_none() || udt.auth.is_none() || name.auth == udt.auth)
}

/// The `<method characteristic>`s of an original method specification as
/// written, each with where it is.
#[derive(Default)]
struct Written {
    /// The `<language clause>`: whether SQL.
    language: Option<(bool, usize)>,
    /// The `<parameter style clause>`.
    style: Option<usize>,
    /// The `<deterministic characteristic>`.
    deterministic: Option<(bool, usize)>,
    /// The `<SQL-data access indication>`: none for NO SQL.
    access: Option<(Option<DataAccess>, usize)>,
    /// The `<null-call clause>`.
    null_call: Option<(bool, usize)>,
}

/// Puts `value`, written at `at`, in `slot`, unless one is there: at most
/// one of each characteristic \[§11.51 SR9b xiv 1, §11.56 SR9\].
fn once<T>(slot: &mut Option<T>, value: T, (at, defined): (usize, Defined)) -> Result<bool> {
    if slot.is_some() {
        let rule = defined.of(("§11.51 SR9b xiv 1", "§11.56 SR9"));
        return Err(broken(rule, at, "the <method characteristics> contain one characteristic twice"));
    }
    *slot = Some(value);
    Ok(true)
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §11.51] `<member list>` ::= ( `<member>` \[{,
    /// `<member>`}...\] ), each an `<attribute definition>`, no two names
    /// equivalent \[SR8f\].
    pub(super) fn member_list(&mut self) -> Result<Vec<AttributeDefinition>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut members: Vec<AttributeDefinition> = Vec::new();
        loop {
            let member = self.attribute_definition()?;
            if members.iter().any(|m| m.name == member.name) {
                let msg = format!("the type has two attributes {}", member.name);
                return Err(broken("§11.51 SR8f", member.at, &msg));
            }
            members.push(member);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(members)
    }

    /// [ISO/IEC 9075-2:2023 §11.52] `<attribute definition>` ::=
    /// `<attribute name>` `<data type>` \[`<attribute default>`\]
    /// \[`<collate clause>`\], a `<collate clause>` not both in the type and
    /// after it \[SR4\]. The text of the `<default option>` is kept.
    pub(super) fn attribute_definition(&mut self) -> Result<AttributeDefinition> {
        let at = self.here();
        let name = self.identifier()?;
        let data_type = self.type_name()?;
        let in_type = self.collate_clause()?;
        let (default, default_text) = if self.accept_kw(kw::DEFAULT) {
            let start = self.here();
            let option = self.default_option()?;
            (Some(option), Some(self.text_from(start)))
        } else {
            (None, None)
        };
        let collate_at = self.here();
        let after = self.collate_clause()?;
        if in_type.is_some() && after.is_some() {
            return Err(broken("§11.52 SR4", collate_at, "a <collate clause> is in the <data type> and after it"));
        }
        Ok(AttributeDefinition { name, data_type, default, default_text, collate: in_type.or(after), at })
    }

    /// Whether a `<method specification>` begins here.
    pub(super) fn at_method_specification(&self) -> bool {
        METHOD_WORDS.iter().any(|w| self.is_word(w))
    }

    /// [ISO/IEC 9075-2:2023 §11.51] `<method specification list>` ::=
    /// `<method specification>` \[{, `<method specification>`}...\], of the
    /// type `udt`.
    pub(super) fn method_specification_list(&mut self, udt: &TableName) -> Result<Vec<MethodSpecification>> {
        let mut methods = vec![self.method_specification(udt, Defined::Type)?];
        while self.accept_delim(delim::COMMA) {
            methods.push(self.method_specification(udt, Defined::Type)?);
        }
        Ok(methods)
    }

    /// [ISO/IEC 9075-2:2023 §11.51] `<method specification>` ::=
    /// `<original method specification>` | `<overriding method
    /// specification>`, of the type `udt`, read where `defined` says.
    pub(super) fn method_specification(&mut self, udt: &TableName, defined: Defined) -> Result<MethodSpecification> {
        let at = self.here();
        let overriding = self.accept_word(OVERRIDING);
        let kind = self.method_kind();
        self.expect_word(METHOD)?;
        let name = self.identifier()?;
        let parameters = self.method_parameters(defined, !overriding)?;
        self.expect_word(RETURNS)?;
        let returns = self.method_returns(defined)?;
        let specific = if self.accept_word(SPECIFIC) { Some(self.table_name()?) } else { None };
        let mut spec = MethodSpecification {
            overriding,
            kind,
            name,
            parameters,
            returns,
            self_as_result: false,
            specific,
            characteristics: MethodCharacteristics::default(),
            at,
        };
        if !overriding {
            spec.self_as_result = self.self_as()?;
            spec.characteristics = self.method_characteristics(defined)?;
        }
        check_method(&spec, udt, defined)?;
        Ok(spec)
    }

    /// The `<returns data type>` of a method specification, with no
    /// `<locator indication>` and no `<result cast>`, which are of an
    /// external method alone \[§11.51 SR9b x, xiv 6 A I; §11.56 SR9b i 4,
    /// SR13a; §11.57 SR8\].
    fn method_returns(&mut self, defined: Defined) -> Result<TypeName> {
        let returns = self.type_name()?;
        let at = self.here();
        if self.is_kw(kw::AS) && matches!(self.peek_at(1), Tok::Identifier(w) if w == LOCATOR) {
            let rule = defined.of(("§11.51 SR9b x 1", "§11.56 SR13a"));
            return Err(broken(rule, at, "the <returns data type> of an SQL method has no <locator indication>"));
        }
        if self.is_kw(kw::CAST) {
            let rule = defined.of(("§11.51 SR9b xiv 6 A I", "§11.56 SR9b i 4"));
            return Err(broken(rule, at, "the <returns clause> of an SQL method has no <result cast>"));
        }
        Ok(returns)
    }

    /// \[SELF AS RESULT\] \[SELF AS LOCATOR\]: whether the former. SELF AS
    /// LOCATOR is refused: locators of user-defined types are Feature
    /// S231's, which turborel does not support.
    fn self_as(&mut self) -> Result<bool> {
        let mut result = false;
        while self.is_word(SELF) && self.next_is_kw(kw::AS) {
            let at = self.here();
            self.advance();
            self.advance();
            if !result && self.accept_word(RESULT) {
                result = true;
                continue;
            }
            if self.is_word(LOCATOR) {
                let msg = "[ISO/IEC 9075-2:2023 §11.51] SELF AS LOCATOR needs Feature S231, “Structured type locators”, which turborel does \
                           not support";
                return Err(SqlError::at(Code::Unsupported, at, msg));
            }
            return Err(self.syntax("expected RESULT or LOCATOR"));
        }
        Ok(result)
    }

    /// [ISO/IEC 9075-2:2023 §11.51] `<SQL parameter declaration list>` of
    /// a method specification, read where `defined` says, each parameter
    /// named if `named`: an original specification's are \[SR9b xiv 6 A
    /// IV\].
    pub(super) fn method_parameters(&mut self, defined: Defined, named: bool) -> Result<Vec<ParameterDeclaration>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut parameters: Vec<ParameterDeclaration> = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(parameters);
        }
        loop {
            let parameter = self.method_parameter(defined, named)?;
            let twice = !parameter.name.is_empty() && parameters.iter().any(|p| p.name == parameter.name);
            if twice {
                let rule = defined.of(("§11.51 SR9b viii 1", "§11.56 SR10"));
                return Err(broken(rule, parameter.at, &format!("SQL parameter {} is declared twice", parameter.name)));
            }
            parameters.push(parameter);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(parameters)
    }

    /// One `<SQL parameter declaration>` of a method: no `<parameter
    /// mode>`, no name SELF, no ROW, no `<locator indication>` and no
    /// RESULT \[§11.51 SR9b viii, ix; §11.56 SR11, SR12\].
    fn method_parameter(&mut self, defined: Defined, named: bool) -> Result<ParameterDeclaration> {
        let at = self.here();
        if self.parameter_mode().is_some() {
            return Err(broken(defined.of(("§11.51 SR9b ix 1", "§11.56 SR12a")), at, "a parameter of a method has no <parameter mode>"));
        }
        let name = if self.at_unnamed_parameter() {
            if named {
                let rule = defined.of(("§11.51 SR9b xiv 6 A IV", "§11.56 SR9b i 3"));
                return Err(broken(rule, at, "every SQL parameter of an SQL method is named"));
            }
            String::new()
        } else {
            self.identifier()?
        };
        if name == SELF {
            return Err(broken(defined.of(("§11.51 SR9b viii 2", "§11.56 SR11")), at, "no SQL parameter of a method is named SELF"));
        }
        if self.at_row_type() {
            return Err(broken(defined.of(("§11.51 SR9b ix 3", "§11.56 SR12c")), self.here(), "a parameter of a method is not of ROW"));
        }
        let data_type = self.type_name()?;
        if self.is_kw(kw::AS) {
            let rule = defined.of(("§11.51 SR9b ix 4 A", "§11.56 SR12d i"));
            return Err(broken(rule, self.here(), "a parameter of an SQL method has no <locator indication>"));
        }
        if self.is_word(RESULT) {
            return Err(broken(defined.of(("§11.51 SR9b ix 2", "§11.56 SR12b")), self.here(), "a parameter of a method is not RESULT"));
        }
        Ok(ParameterDeclaration { mode: None, name, data_type: ParameterType::Data(data_type), default: None, at })
    }

    /// [ISO/IEC 9075-2:2023 §11.51] `<method characteristics>`, in any
    /// order, each at most once \[SR9b xiv 1\], checked: of LANGUAGE SQL,
    /// written or implicit \[SR9b xiv 2\], no `<parameter style clause>`
    /// and not NO SQL \[SR9b xiv 6 A\]; an `<SQL-data access indication>`
    /// written \[SR9b xiv 4\], or of ALTER TYPE CONTAINS SQL if none is
    /// \[§11.56 SR9d\].
    fn method_characteristics(&mut self, defined: Defined) -> Result<MethodCharacteristics> {
        let mut written = Written::default();
        while self.method_characteristic(&mut written, defined)? {}
        if let Some((false, at)) = written.language {
            let msg = "[ISO/IEC 9075-2:2023 §11.51 SR9b xiv 6 B] a method of a language other than SQL is an external method, which \
                       turborel does not support";
            return Err(SqlError::at(Code::Unsupported, at, msg));
        }
        if let Some(at) = written.style {
            let rule = defined.of(("§11.51 SR9b xiv 6 A III", "§11.56 SR9b i 1"));
            return Err(broken(rule, at, "an SQL method has no <parameter style clause>"));
        }
        let access = match written.access {
            Some((Some(access), _)) => access,
            Some((None, at)) => {
                return Err(broken(defined.of(("§11.51 SR9b xiv 6 A II", "§11.56 SR9b i 2")), at, "an SQL method is not NO SQL"));
            }
            None if defined == Defined::Altered => DataAccess::Contains,
            None => {
                let msg = "the <method characteristics> of an original method specification contain an <SQL-data access indication>";
                return Err(broken("§11.51 SR9b xiv 4", self.here(), msg));
            }
        };
        let deterministic = written.deterministic.is_some_and(|(d, _)| d);
        let null_call = written.null_call.is_some_and(|(n, _)| n);
        Ok(MethodCharacteristics { deterministic, access, null_call })
    }

    /// One `<method characteristic>`, put in `written`, if one is next.
    fn method_characteristic(&mut self, written: &mut Written, defined: Defined) -> Result<bool> {
        let at = self.here();
        if self.accept_kw(kw::LANGUAGE) {
            let sql = self.accept_kw(kw::SQL);
            if !sql {
                self.identifier()?;
            }
            return once(&mut written.language, (sql, at), (at, defined));
        }
        if self.accept_word(PARAMETER) {
            self.expect_word(STYLE)?;
            if !self.accept_kw(kw::SQL) {
                self.expect_word(GENERAL)?;
            }
            return once(&mut written.style, at, (at, defined));
        }
        if let Some(deterministic) = self.deterministic_characteristic()? {
            return once(&mut written.deterministic, (deterministic, at), (at, defined));
        }
        if let Some(access) = self.data_access_indication()? {
            let access = match access {
                Access::NoSql => None,
                Access::Of(access) => Some(access),
            };
            return once(&mut written.access, (access, at), (at, defined));
        }
        if let Some(null_call) = self.null_call_clause()? {
            return once(&mut written.null_call, (null_call, at), (at, defined));
        }
        Ok(false)
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b ii, iii, xi; §11.56 SR5, SR6, SR14;
/// §11.57 SR2] The rules on a method specification `spec` of `udt` that
/// need no catalog: a static method neither SELF AS RESULT nor
/// overriding; a constructor SELF AS RESULT, not overriding, named and
/// returning its type; and SELF AS RESULT of a method returning its type.
fn check_method(spec: &MethodSpecification, udt: &TableName, defined: Defined) -> Result<()> {
    let at = spec.at;
    match spec.kind {
        MethodKind::Static if spec.overriding => {
            return Err(broken(defined.of(("§11.51 SR9b ii 1", "§11.57 SR2")), at, "a static method is not overriding"));
        }
        MethodKind::Static if spec.self_as_result => {
            return Err(broken(defined.of(("§11.51 SR9b ii 1", "§11.56 SR6a")), at, "a static method is not SELF AS RESULT"));
        }
        MethodKind::Constructor if spec.overriding => {
            return Err(broken(defined.of(("§11.51 SR9b iii 2", "§11.57 SR2")), at, "a constructor method is not overriding"));
        }
        MethodKind::Constructor if !spec.self_as_result => {
            return Err(broken(defined.of(("§11.51 SR9b iii 1", "§11.56 SR5a")), at, "a constructor method is SELF AS RESULT"));
        }
        MethodKind::Constructor if spec.name != udt.ident => {
            let msg = format!("a constructor method is named as its type {}, not {}", udt.ident, spec.name);
            return Err(broken(defined.of(("§11.51 SR9b iii 3", "§11.56 SR5b")), at, &msg));
        }
        MethodKind::Constructor if !names_type(&spec.returns, udt) => {
            let msg = format!("a constructor method returns its type {}", udt.ident);
            return Err(broken(defined.of(("§11.51 SR9b iii 4", "§11.56 SR5c")), at, &msg));
        }
        MethodKind::Static | MethodKind::Constructor | MethodKind::Instance => {}
    }
    if spec.self_as_result && !names_type(&spec.returns, udt) {
        let msg = format!("a method SELF AS RESULT returns its type {}", udt.ident);
        return Err(broken(defined.of(("§11.51 SR9b xi", "§11.56 SR14")), at, &msg));
    }
    Ok(())
}

// Unit tests: each form of the statements of structured types read, and
// each Syntax Rule refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
