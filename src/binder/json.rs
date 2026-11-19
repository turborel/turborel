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
//! `PASSING 1 AS x` is `$X`, and `PASSING 1 AS "x"` is `$x`. Each is a
//! potential source of non-determinism [§9.16 SR1g, SR1h, SR1al], so a
//! constraint or a generation expression does not contain one.
//!
//! The constructors, the aggregates and IS JSON are bound in
//! `binder/json/construct.rs`.

mod accessor;
mod construct;
mod lazy;
mod table;
mod typed;

pub(super) use accessor::Step;
pub(super) use table::json_table_item;

use std::sync::Arc;

use super::expression::nondeterministic_source;
use super::{Binder, Purpose, Scope, rule};
use crate::ast::{JsonClauses, JsonExpression, JsonOperator, JsonValueBehavior, TypeName, ValueExpression};
use crate::error::{At, Result};
use crate::functions::{
    JSON_EXISTS, JSON_QUERY, JSON_VALUE, JsonCall, JsonOperation, ScalarRef, ValueSlot, input_type, json_text, output_type,
};
use crate::json::{
    Accessor, ExistsBehavior, Expr as PathExpr, IMPLICIT_RETURNING, Path, Predicate, QueryBehavior, Quotes, Wrapper, parse_path,
};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [§6.28, §6.34, §6.35] The SQL/JSON function `j` that is a value: a
    /// query operator, or a constructor (`binder/json/construct.rs`).
    pub(super) fn json_expression(&self, j: &JsonExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match j {
            JsonExpression::Query(operator) => self.json_operator(operator, scope),
            JsonExpression::Constructor(constructor) => self.json_constructor(constructor, scope),
            JsonExpression::Typed(typed) => self.json_typed(typed, scope),
        }
    }

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
        // [§9.16 SR1g, SR1h, SR1al] each a potential source of
        // non-determinism, which a constraint does not contain; an index's
        // key part only where it is safe (RULES.md D92).
        if self.purpose.constrains() && self.purpose != Purpose::Key {
            let (item, name) = match &j.clauses {
                JsonClauses::Exists { .. } => ("al", JSON_EXISTS),
                JsonClauses::Value { .. } => ("g", JSON_VALUE),
                JsonClauses::Query { .. } => ("h", JSON_QUERY),
            };
            return Err(nondeterministic_source(item, name));
        }
        let common = &j.common;
        let path = parse_path(&common.path, self.now).or_at(common.path_at)?;
        let names: Vec<&str> = common.passing.iter().map(|argument| argument.name.as_str()).collect();
        path.check_variables(&names).or_at(common.path_at)?;
        let (operation, defaults) = operation(&j.clauses)?;
        let passing: Vec<(String, bool)> =
            common.passing.iter().map(|argument| (argument.name.clone(), argument.input.format.is_some())).collect();
        let f = ScalarRef(Arc::new(JsonCall::new(&common.path, path.clone(), passing.clone(), operation.clone())));
        let operands: Vec<ValueExpression> = std::iter::once(&common.context.value)
            .chain(common.passing.iter().map(|argument| &argument.input.value))
            .chain(defaults)
            .cloned()
            .collect();
        let (exprs, types) = self.arguments(&*f.0, &operands, scope)?;
        self.json_operands(j, &path, &exprs, &types)?;
        let inputs = types.get(..=common.passing.len()).unwrap_or_default();
        let (f, operation) = match json_returned(&operation, inputs)? {
            Some(json) => (ScalarRef(Arc::new(JsonCall::new(&common.path, path.clone(), passing.clone(), json.clone()))), json),
            None => (f, operation),
        };
        let t = f.0.return_type(&types)?;
        // [§9.48 GR4c iii, GR5d] A DEFAULT evaluated only when taken.
        let bound = lazy::Bound { text: &common.path, path: &path, passing: &passing, operation: &operation };
        if let Some(lazy) = self.lazy_value(&bound, &exprs, &types)? {
            return Ok((lazy, t));
        }
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
            input_type(t, argument.input.format, PASSED).or_at(argument.at)?;
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
                returning: output_type(returning.as_ref(), JSON_QUERY)?,
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

/// [§6.35 SR1a] JSON_QUERY's `operation` returning JSON, if it returns
/// the implicit type and the context item or an argument, of `inputs`,
/// is of the JSON type; none if it is unchanged. Its returned type is
/// `IMPLICIT_RETURNING` only if no `<JSON output clause>` is written.
///
/// # Errors
///
/// SQLCODE -103 if it returns JSON and OMIT QUOTES is written \[SR3\].
fn json_returned(operation: &JsonOperation, inputs: &[DataType]) -> Result<Option<JsonOperation>> {
    let JsonOperation::Query { returning, wrapper, quotes, on_empty, on_error } = operation else { return Ok(None) };
    let implicit = *returning == IMPLICIT_RETURNING && inputs.iter().any(|t| matches!(t, DataType::Json));
    if (implicit || *returning == DataType::Json) && *quotes == Quotes::Omit {
        return Err(rule("[ISO/IEC 9075-2:2023 §6.35 SR3] JSON_QUERY RETURNING JSON keeps its quotes, KEEP QUOTES"));
    }
    let json =
        JsonOperation::Query { returning: DataType::Json, wrapper: *wrapper, quotes: *quotes, on_empty: *on_empty, on_error: *on_error };
    Ok(implicit.then_some(json))
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
        TypeName::Domain(_)
        | TypeName::Reference(..)
        | TypeName::Collection(..)
        | TypeName::Row(_)
        | TypeName::Extension { .. }
        | TypeName::XmlSchema(..) => Err(rule(VALUE_RULE)),
    }
}

/// The Syntax Rule a type JSON_VALUE does not return breaks.
const VALUE_RULE: &str =
    "[ISO/IEC 9075-2:2023 §6.28 SR2] JSON_VALUE returns a <predefined type> of a character string, a number, a truth value or a datetime";

/// [§6.28 SR2] Whether JSON_VALUE returns the type `t`.
fn returnable(t: &DataType) -> bool {
    t.is_character() || t.is_numeric() || t.is_datetime() || matches!(t, DataType::Boolean)
}

/// The Syntax Rule a context item of another type than a string's breaks.
const CONTEXT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.14 SR1] the context item of an SQL/JSON query operator is of a string type";
/// What a PASSING argument is, in the errors of its type.
const PASSED: &str = "a PASSING argument";

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

#[cfg(test)]
mod tests;
