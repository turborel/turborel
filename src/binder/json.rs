//! [ISO/IEC 9075-2:2023 §4.48.4] The SQL/JSON query operators bound:
//! JSON_EXISTS [§8.23], a BOOLEAN that `Cond::Value` judges, and
//! JSON_VALUE [§6.28] and JSON_QUERY [§6.35], each a call of a function of
//! its own (`JsonCall`, RULES.md D84).
//!
//! The path specification is parsed here, once for the statement, so a
//! path that is none is refused as the statement is prepared, and the
//! Syntax Rules that need the PASSING clause, §9.46 SR4 and SR8, are
//! checked of the parsed path. A PASSING argument's `<identifier>` is
//! equivalent to a named variable's `<JSON path identifier>` \[SR4\] if its
//! value, a `<regular identifier>` in upper case or a `<delimited
//! identifier>`'s body [ISO/IEC 9075:1992 §5.2 SR10, SR11], is the
//! variable's name, which the path language writes case-sensitively: so
//! `PASSING 1 AS x` is `$X`, and `PASSING 1 AS "x"` is `$x`.

use std::sync::Arc;

use super::{Binder, Scope, rule};
use crate::ast::{JsonClauses, JsonFormat, JsonInput, JsonOperator, JsonValueBehavior, TypeName, ValueExpression};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{JsonCall, JsonOperation, ScalarRef, ValueSlot};
use crate::json::{
    Accessor, ExistsBehavior, Expr as PathExpr, IMPLICIT_RETURNING, Path, Predicate, QueryBehavior, Quotes, Wrapper, parse_path,
};
use crate::logical::Expr;
use crate::value::DataType;
use crate::value::lob::LobKind;

impl Binder<'_> {
    /// [§8.23, §6.28, §6.35] The SQL/JSON query operator `j`: a call of
    /// its function, and its declared type.
    ///
    /// # Errors
    ///
    /// The path specification is no path expression [§9.46 SR3, SR5 to
    /// SR7]; SQLCODE -103 if it names a variable no argument has \[SR4\], an
    /// operand's or the returned type is not one the operator takes, or a
    /// DEFAULT is not cast to the returned type; -900 of a binary string's
    /// JSON text, or a value or a type turborel does not implement here.
    pub(super) fn json_operator(&self, j: &JsonOperator, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let common = &j.common;
        let path = parse_path(&common.path, self.now).or_at(common.path_at)?;
        let names: Vec<&str> = common.passing.iter().map(|argument| argument.name.as_str()).collect();
        path.check_variables(&names).or_at(common.path_at)?;
        let (operation, defaults) = operation(&j.clauses)?;
        let passing = common.passing.iter().map(|argument| (argument.name.clone(), argument.input.format.is_some())).collect();
        let f = ScalarRef(Arc::new(JsonCall::new(&common.path, path.clone(), passing, operation)));
        let operands: Vec<ValueExpression> = std::iter::once(&common.context.value)
            .chain(common.passing.iter().map(|argument| &argument.input.value))
            .chain(defaults)
            .cloned()
            .collect();
        let (exprs, types) = self.arguments(&*f.0, &operands, scope)?;
        self.json_operands(j, &path, &exprs, &types)?;
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// The operands of `j`, bound to `exprs` of `types`, checked by the
    /// Syntax Rules, its path parsed to `path`: the context item's
    /// [§10.14 SR1], each argument's [§10.12 SR3, SR6 to SR8, §9.46 SR8],
    /// and each DEFAULT's, which is cast to the returned type [§9.48 GR4c
    /// iii, GR5d].
    fn json_operands(&self, j: &JsonOperator, path: &Path, exprs: &[Expr], types: &[DataType]) -> Result<()> {
        // The first type is the context item's, the next the arguments'.
        types.iter().take(1).try_for_each(|context| json_text(context, j.common.context.format, CONTEXT_RULE))?;
        for (argument, t) in j.common.passing.iter().zip(types.iter().skip(1)) {
            argument_type(t, &argument.input).or_at(argument.at)?;
        }
        starts_with_arguments(j, path, types)?;
        let JsonClauses::Value { returning, .. } = &j.clauses else { return Ok(()) };
        let to = value_type(returning.as_ref())?;
        let defaults = exprs.iter().zip(types).skip(j.common.passing.len().saturating_add(1));
        for (e, t) in defaults {
            self.cast_bound((e.clone(), t.clone()), to.clone())?;
        }
        Ok(())
    }
}

/// The operation of the clauses `clauses`, each explicit or implicit, and
/// the `<value expression>`s of its DEFAULTs, ON EMPTY's first.
///
/// # Errors
///
/// SQLCODE -103 if the returned type is not one the operator returns; -900
/// if it is a binary string type.
fn operation(clauses: &JsonClauses) -> Result<(JsonOperation, Vec<&ValueExpression>)> {
    match clauses {
        // [§8.23 SR1] FALSE ON ERROR if none is written.
        JsonClauses::Exists { on_error } => Ok((JsonOperation::Exists(on_error.unwrap_or(ExistsBehavior::False)), Vec::new())),
        JsonClauses::Value { returning, on_empty, on_error } => {
            // [§6.28 SR4, SR5] NULL ON EMPTY and NULL ON ERROR if none is
            // written.
            let (on_empty, on_error) = (on_empty.as_ref(), on_error.as_ref());
            let defaults = [on_empty, on_error].into_iter().flatten().filter_map(default_of).collect();
            let operation =
                JsonOperation::Value { returning: value_type(returning.as_ref())?, on_empty: slot(on_empty), on_error: slot(on_error) };
            Ok((operation, defaults))
        }
        JsonClauses::Query { returning, wrapper, quotes, on_empty, on_error } => {
            let operation = JsonOperation::Query {
                returning: query_type(returning.as_ref())?,
                // [§6.35 SR6] WITHOUT ARRAY if none is written.
                wrapper: wrapper.unwrap_or(Wrapper::Without),
                // [§6.35 SR9] KEEP if none is written.
                quotes: quotes.unwrap_or(Quotes::Keep),
                // [§6.35 SR4, SR5] NULL ON EMPTY and NULL ON ERROR.
                on_empty: on_empty.unwrap_or(QueryBehavior::Null),
                on_error: on_error.unwrap_or(QueryBehavior::Null),
            };
            Ok((operation, Vec::new()))
        }
    }
}

/// [§6.28 SR4, SR5] The slot of the behavior `written`, NULL if none is.
fn slot(written: Option<&JsonValueBehavior>) -> ValueSlot {
    match written {
        None | Some(JsonValueBehavior::Null) => ValueSlot::Null,
        Some(JsonValueBehavior::Error) => ValueSlot::Error,
        Some(JsonValueBehavior::Default(_)) => ValueSlot::Default,
    }
}

/// The `<value expression>` of `behavior`, if it is DEFAULT.
fn default_of(behavior: &JsonValueBehavior) -> Option<&ValueExpression> {
    match behavior {
        JsonValueBehavior::Default(value) => Some(value),
        JsonValueBehavior::Error | JsonValueBehavior::Null => None,
    }
}

/// [§6.28 SR1, SR2] The type JSON_VALUE returns: that of its `<JSON
/// returning clause>`, `written`, a `<predefined type>` of a character
/// string, a number, a truth value or a datetime; without one, the
/// implementation-defined `IMPLICIT_RETURNING` (ID074).
///
/// # Errors
///
/// SQLCODE -103 if `written` is of no such type \[SR2\].
fn value_type(written: Option<&TypeName>) -> Result<DataType> {
    let Some(written) = written else { return Ok(IMPLICIT_RETURNING) };
    match written {
        TypeName::Data(t) => {
            if returnable(t) {
                Ok(t.clone())
            } else {
                Err(rule(format!("{VALUE_RULE}, not {t}")))
            }
        }
        TypeName::Domain(_) | TypeName::Reference(..) | TypeName::Collection(..) | TypeName::Row(_) | TypeName::Extension { .. } => {
            Err(rule(VALUE_RULE))
        }
    }
}

/// The Syntax Rule a type JSON_VALUE does not return breaks.
const VALUE_RULE: &str =
    "[ISO/IEC 9075-2:2023 §6.28 SR2] JSON_VALUE returns a <predefined type> of a character string, a number, a truth value or a datetime";

/// [§6.28 SR2] Whether JSON_VALUE returns the type `t`.
fn returnable(t: &DataType) -> bool {
    t.is_character() || t.is_numeric() || t.is_datetime() || matches!(t, DataType::Boolean)
}

/// [§6.35 SR1b, §10.13 SR2] The type JSON_QUERY returns: that of its
/// `<JSON output clause>`, `written`, a character string type, FORMAT JSON
/// explicit or implicit \[SR1\]; without one, the implementation-defined
/// `IMPLICIT_RETURNING` (ID101).
///
/// # Errors
///
/// SQLCODE -103 if `written` is of no string type \[SR2\] or a character
/// string type of ENCODING \[SR2a\]; -900 of a binary string type, whose
/// encodings turborel does not implement.
fn query_type(written: Option<&(TypeName, Option<JsonFormat>)>) -> Result<DataType> {
    let Some((written, format)) = written else { return Ok(IMPLICIT_RETURNING) };
    let TypeName::Data(t) = written else {
        return Err(rule("[ISO/IEC 9075-2:2023 §10.13 SR2] the <JSON output clause> of JSON_QUERY is of a string type"));
    };
    if t.is_binary() {
        return Err(binary(&format!("JSON_QUERY RETURNING {t}")));
    }
    if !t.is_character() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.13 SR2] the <JSON output clause> of JSON_QUERY is of a string type, not {t}")));
    }
    if format.is_some_and(|f| f.encoding.is_some()) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.13 SR2a] JSON_QUERY RETURNING {t}, a character string type, has no ENCODING")));
    }
    Ok(t.clone())
}

/// The Syntax Rule a context item of another type than a string's breaks.
const CONTEXT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.14 SR1] the context item of an SQL/JSON query operator is of a string type";
/// The Syntax Rule an argument FORMAT JSON of another type breaks.
const FORMATTED_RULE: &str = "[ISO/IEC 9075-2:2023 §10.12 SR6] a PASSING argument FORMAT JSON is of a string type";

/// [§10.12 SR6, SR8] That JSON text of the type `t`, with the `<JSON input
/// clause>` `format`, is a character string's, of no ENCODING; `cited` is
/// the rule that makes it a string.
///
/// # Errors
///
/// SQLCODE -900 if `t` is a binary string type, whose encodings turborel
/// does not implement; -103 if it is of no string type, or ENCODING is
/// written \[SR8\].
fn json_text(t: &DataType, format: Option<JsonFormat>, cited: &str) -> Result<()> {
    if t.is_binary() {
        return Err(binary(&format!("JSON text of {t}")));
    }
    if !t.is_character() {
        return Err(rule(format!("{cited}, not {t}")));
    }
    if format.is_some_and(|f| f.encoding.is_some()) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR8] JSON text of {t}, a character string type, has no ENCODING")));
    }
    Ok(())
}

/// [§10.12 SR3, SR6 to SR8] That the PASSING argument `input`, of type
/// `t`, is of a type it may be: FORMAT JSON, a character string's JSON
/// text; else a character string, a number, a datetime, a truth value or
/// an interval, which is cast to a character string [§9.46 GR11b i 2 B].
///
/// # Errors
///
/// As `json_text` of an argument FORMAT JSON; else SQLCODE -103 of a
/// binary string \[SR7\], an array, a multiset or a row, which CAST takes
/// to no character string \[SR3\]; -900 of a user-defined type, a
/// reference or a type beyond the standard, whose casts turborel does not
/// apply here.
fn argument_type(t: &DataType, input: &JsonInput) -> Result<()> {
    if input.format.is_some() {
        return json_text(t, input.format, FORMATTED_RULE);
    }
    match t {
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Lob { kind: LobKind::Character, .. } => Ok(()),
        DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Lob { kind: LobKind::Binary, .. } => {
            Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR7] a PASSING argument of the binary string type {t} is FORMAT JSON")))
        }
        DataType::Array(_) | DataType::Multiset(_) | DataType::Row(_) => {
            Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR3] a PASSING argument is of a type CAST takes to a character string, not {t}")))
        }
        DataType::Distinct(_) | DataType::Structured(_) | DataType::Reference(_) | DataType::Extension(_) => Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] a PASSING argument of {t}, cast to a character string, is valid SQL but is not implemented (RULES.md D18)"),
        )),
    }
}

/// The error of `what`, of a binary string type, valid but not
/// implemented (RULES.md D18).
fn binary(what: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[DESIGN] {what}, a binary string type, FORMAT JSON ENCODING, is valid SQL but is not implemented (RULES.md D18)"),
    )
}

/// [§9.46 SR8] That each named variable of `path` that is a `<JSON starts
/// with initial>` is equivalent to an argument of `j` that is a character
/// string, of `types` after the context item's, without FORMAT JSON: the
/// first of its name, as the path takes it \[GR11b\].
///
/// # Errors
///
/// SQLCODE -103 if one is not.
fn starts_with_arguments(j: &JsonOperator, path: &Path, types: &[DataType]) -> Result<()> {
    let mut initials = Vec::new();
    expr_initials(&path.expr, &mut initials);
    for name in initials {
        let argument = j.common.passing.iter().zip(types.iter().skip(1)).find(|(argument, _)| argument.name == name);
        let Some((argument, t)) = argument else { continue };
        if argument.input.format.is_some() || !t.is_character() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.46 SR8] the variable ${name} after starts with is a character string argument without FORMAT JSON"
            ))
            .or_at(argument.at));
        }
    }
    Ok(())
}

/// [§9.46 SR8] The names of the named variables of `expr` that are a
/// `<JSON starts with initial>`, pushed to `names`. A parsed path is at
/// most the path module's `MAX_PATH_NESTING` nodes deep, so the recursion is
/// bounded (RULES.md R-CF-6).
fn expr_initials<'p>(expr: &'p PathExpr, names: &mut Vec<&'p str>) {
    match expr {
        PathExpr::Literal(_) | PathExpr::Context | PathExpr::Variable(_) | PathExpr::Current | PathExpr::Last => {}
        PathExpr::Access(base, op) => {
            expr_initials(base, names);
            accessor_initials(op, names);
        }
        PathExpr::Signed(_, operand) => expr_initials(operand, names),
        PathExpr::Arithmetic(left, _, right) => {
            expr_initials(left, names);
            expr_initials(right, names);
        }
    }
}

/// `expr_initials` of the wffs of an accessor op.
fn accessor_initials<'p>(op: &'p Accessor, names: &mut Vec<&'p str>) {
    match op {
        Accessor::Member(_) | Accessor::AnyMember | Accessor::AnyElement | Accessor::Method(_) => {}
        Accessor::Subscripts(list) => {
            for subscript in list {
                expr_initials(&subscript.from, names);
                subscript.to.iter().for_each(|to| expr_initials(to, names));
            }
        }
        Accessor::Filter(predicate) => predicate_initials(predicate, names),
    }
}

/// `expr_initials` of a predicate's wffs, and the initial of a `<JSON
/// starts with predicate>` that is a named variable.
fn predicate_initials<'p>(predicate: &'p Predicate, names: &mut Vec<&'p str>) {
    match predicate {
        Predicate::StartsWith(whole, initial) => {
            expr_initials(whole, names);
            if let PathExpr::Variable(name) = initial {
                names.push(name);
            }
        }
        Predicate::Exists(expr) | Predicate::LikeRegex(expr, _) => expr_initials(expr, names),
        Predicate::Compare(left, _, right) => {
            expr_initials(left, names);
            expr_initials(right, names);
        }
        Predicate::IsUnknown(operand) | Predicate::Not(operand) => predicate_initials(operand, names),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_initials(left, names);
            predicate_initials(right, names);
        }
    }
}
