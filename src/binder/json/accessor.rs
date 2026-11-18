//! [ISO/IEC 9075-2:2023 §6.36] The `<JSON simplified accessor>` bound, as
//! the JSON_VALUE or JSON_QUERY it is equivalent to \[SR3\]: of a value
//! of the JSON type \[SR2\], its op chain JC made the path `lax $JC`.
//!
//! A JSON column is refinable [§6.6 SR6], so an identifier chain whose
//! basis is one is the accessor of its other identifiers \[§6.6 SR11\]:
//! `j.a.b` and `t.j.a` are member accessors, one at a time, each an
//! `<identifier>` of SQL, so a regular one is upper case and a delimited
//! one as written (`refined`); an element reference `j[n]` an array
//! accessor of the value of `n`; and `j.m(...)` an item method. A chain
//! written with an op SQL has none, `.*`, `[*]` or a `<JSON path string
//! literal>` member name, is read whole as written, its keys' case kept
//! \[NOTE 198\] (`JsonTyped::Accessor`).
//!
//! \[SR3a\] A last item method of an SQL type gives JSON_VALUE RETURNING
//! it. As printed, SR3a i 7 makes `size()` CHARACTER VARYING and 8 makes
//! `string()` and `type()` BIGINT, the two swapped against what the
//! methods give [§9.46 GR11g ii 6 C]: turborel reads `size()` as BIGINT
//! and `string()`, `type()` and `number()` as CHARACTER VARYING.

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use crate::ast::{Literal, ValueExpression, ValueSpecification};
use crate::error::Result;
use crate::functions::{JsonCall, JsonOperation, ScalarRef, ValueSlot};
use crate::json::{Accessor, Expr as PathExpr, Method, QueryBehavior, Quotes, Wrapper, parse_path};
use crate::logical::Expr;
use crate::value::{
    BIGINT_PRECISION, DEFAULT_EXACT_PRECISION, DOUBLE_PRECISION, DataType, INTEGER_PRECISION, MAX_CHARACTER_LENGTH, Spelled,
};

/// The name of the PASSING variable an element reference's subscript is.
const SUBSCRIPT: &str = "SUBSCRIPT";
/// [§6.1 SR38] The precision of a TIME of none written.
const DEFAULT_TIME_PRECISION: u32 = 0;
/// [§6.1 SR38] The precision of a TIMESTAMP of none written.
const DEFAULT_TIMESTAMP_PRECISION: u32 = 6;
/// [§6.1 SR24] The scale of a DECIMAL of none written.
const DEFAULT_SCALE: u32 = 0;

/// One op of a chain bound one at a time.
#[derive(Clone, Copy)]
pub(in crate::binder) enum Step<'a> {
    /// `.name`: a member accessor of the key `name`.
    Member(&'a str),
    /// `[n]`: an array accessor of the value of `n`.
    Element(&'a ValueExpression),
    /// `.name(args)`: an item method.
    Method(&'a str, &'a [ValueExpression]),
}

impl Binder<'_> {
    /// [§6.36] The accessor of `primary`, a value of the JSON type, and
    /// the op chain `chain` written after it, as the path language writes
    /// it; or, if `sql` is a reading of the same text, that reading,
    /// unless `primary` is a value of the JSON type.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `primary` is of no JSON type \[SR2\] and `sql` is
    /// none; the path's errors [§9.46 SR3].
    pub(super) fn json_accessor(
        &self,
        primary: &ValueExpression,
        chain: &str,
        sql: Option<&ValueExpression>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let bound = self.value_expression(primary, scope);
        if let (Some(sql), false) = (sql, matches!(bound, Ok((_, DataType::Json)))) {
            return self.value_expression(sql, scope);
        }
        self.accessed(bound?, chain, None)
    }

    /// [§6.36] `bound`, of the JSON type, accessed by `step`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of a method's argument other than a literal of a
    /// number or a character string, which the path language writes.
    pub(in crate::binder) fn json_step(&self, bound: (Expr, DataType), step: Step<'_>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match step {
            Step::Member(key) => self.accessed(bound, &format!(".{}", path_string(key)), None),
            Step::Element(index) => {
                let (i, t) = self.value_expression(index, scope)?;
                if !t.is_numeric() {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §6.36, §9.46 SR3] a JSON array accessor's subscript is a number, not {t}"
                    )));
                }
                self.accessed(bound, &format!("[${SUBSCRIPT}]"), Some((i, t)))
            }
            Step::Method(name, args) => {
                let written: Vec<String> = args.iter().map(method_argument).collect::<Result<_>>()?;
                self.accessed(bound, &format!(".{}({})", name.to_lowercase(), written.join(",")), None)
            }
        }
    }

    /// [§6.36 SR3] JSON_VALUE or JSON_QUERY of `bound` and the path `lax
    /// $chain`, its variable SUBSCRIPT `subscript` if one is passed.
    fn accessed(&self, (e, t): (Expr, DataType), chain: &str, subscript: Option<(Expr, DataType)>) -> Result<(Expr, DataType)> {
        if t != DataType::Json {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §6.36 SR2] a JSON simplified accessor is of a value of the JSON type, not {t}"
            )));
        }
        let text = format!("lax ${chain}");
        let path = parse_path(&text, self.now)?;
        let operation = match last_method(&path.expr).and_then(method_type) {
            // [SR3a ii] NULL ON EMPTY NULL ON ERROR.
            Some(returning) => JsonOperation::Value { returning, on_empty: ValueSlot::Null, on_error: ValueSlot::Null },
            // [SR3b] WITH CONDITIONAL ARRAY WRAPPER NULL ON EMPTY NULL ON
            // ERROR, of the JSON type [§6.35 SR1a].
            None => JsonOperation::Query {
                returning: DataType::Json,
                wrapper: Wrapper::Conditional,
                quotes: Quotes::Keep,
                on_empty: QueryBehavior::Null,
                on_error: QueryBehavior::Null,
            },
        };
        let passing = subscript.iter().map(|_| (SUBSCRIPT.to_owned(), false)).collect();
        let f = ScalarRef(Arc::new(JsonCall::new(&text, path, passing, operation)));
        let (mut args, mut types) = (vec![e], vec![t]);
        if let Some((i, it)) = subscript {
            args.push(i);
            types.push(it);
        }
        let returned = f.0.return_type(&types)?;
        Ok((Expr::Call(f, args), returned))
    }
}

/// The method of the last op of `expr`, if it is an item method.
fn last_method(expr: &PathExpr) -> Option<&Method> {
    if let PathExpr::Access(_, Accessor::Method(m)) = expr { Some(m) } else { None }
}

/// [§6.36 SR3a i] The SQL type RT of the item method `m`, if it is one of
/// an SQL type; none of one that gives SQL/JSON items.
fn method_type(m: &Method) -> Option<DataType> {
    let exact = |precision, scale, spelled| DataType::Exact { precision, scale, spelled };
    Some(match m {
        Method::Bigint | Method::Size => exact(BIGINT_PRECISION, 0, Spelled::Bigint),
        Method::Integer => exact(INTEGER_PRECISION, 0, Spelled::Integer),
        Method::Decimal { precision, scale } => {
            exact(precision.unwrap_or(DEFAULT_EXACT_PRECISION), scale.unwrap_or(DEFAULT_SCALE), Spelled::Decimal)
        }
        Method::Double => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision },
        Method::Boolean => DataType::Boolean,
        Method::Date => DataType::Date,
        Method::Number | Method::String | Method::Type => DataType::varying(MAX_CHARACTER_LENGTH),
        Method::Time(p) => DataType::Time { precision: p.unwrap_or(DEFAULT_TIME_PRECISION), zoned: false },
        Method::TimeTz(p) => DataType::Time { precision: p.unwrap_or(DEFAULT_TIME_PRECISION), zoned: true },
        Method::Timestamp(p) => DataType::Timestamp { precision: p.unwrap_or(DEFAULT_TIMESTAMP_PRECISION), zoned: false },
        Method::TimestampTz(p) => DataType::Timestamp { precision: p.unwrap_or(DEFAULT_TIMESTAMP_PRECISION), zoned: true },
        Method::Ceiling | Method::Floor | Method::Abs | Method::Datetime | Method::DatetimeTemplate(_) | Method::Keyvalue => return None,
    })
}

/// [§9.46] `key` as a `<JSON path string literal>`: in quotation marks,
/// each quotation mark and reverse solidus escaped.
fn path_string(key: &str) -> String {
    format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The path language's text of an item method's argument `arg`.
///
/// # Errors
///
/// SQLCODE -103 if it is no literal of a number or a character string.
fn method_argument(arg: &ValueExpression) -> Result<String> {
    if let ValueExpression::Value(ValueSpecification::Literal(literal)) = arg {
        if let Literal::Exact(d) = literal {
            return Ok(d.to_string());
        }
        if let Literal::Character(s) = literal {
            return Ok(path_string(s));
        }
    }
    Err(rule("[ISO/IEC 9075-2:2023 §6.36, §9.46] an item method's argument is a literal of a number or a character string"))
}
