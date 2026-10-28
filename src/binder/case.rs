//! [ISO/IEC 9075:1992 §6.9, §6.10, §7.11, §9.3] CASE, NULLIF and COALESCE,
//! bound as the searched CASE the standard defines each by; CAST; scalar
//! subqueries; and the result type of values brought together. Split from
//! binder/expression.rs (RULES.md R-SIZE-2).

use super::expression::dynamic_of;
use super::subquery::Use;
use super::{Binder, Scope, rule};
use crate::ast::{CaseExpression, Subquery, TypeName, ValueExpression};
use crate::error::{At, Result};
use crate::logical::{Cond, Expr};
use crate::params::ParameterType;
use crate::value::{CompOp, DOUBLE_PRECISION, DataType, MAX_EXACT_PRECISION, Value};

/// [ISO/IEC 9075:1992 §9.3] The result type of values of `types`: CHARACTER
/// of the greatest length, if all are character strings [SR3a ii]; exact,
/// of the greatest scale, if all are exact [SR3c], with as many digits
/// before the point as any has, up to `MAX_EXACT_PRECISION`, the
/// implementation-defined precision; else approximate, of
/// `DOUBLE_PRECISION` [SR3d]. `clause` is the rule to cite.
///
/// # Errors
///
/// SQLCODE -103 if a character string is among numbers [SR2, SR3a].
pub(super) fn set_type(types: &[DataType], clause: &str) -> Result<DataType> {
    let characters = types.iter().filter(|t| t.is_character()).count();
    if characters != 0 && characters != types.len() {
        return Err(rule(format!("[ISO/IEC 9075:1992 {clause}, §9.3 SR2, SR3a] character strings and numbers are brought together")));
    }
    let mut length = 0_u32;
    let (mut whole, mut scale, mut exact) = (0_u32, 0_u32, true);
    for t in types {
        match t {
            DataType::Character { length: n } => length = length.max(*n),
            DataType::Exact { precision, scale: s } => {
                whole = whole.max(precision.saturating_sub(*s));
                scale = scale.max(*s);
            }
            DataType::Approximate { .. } => exact = false,
        }
    }
    Ok(match (characters, exact) {
        (0, true) => DataType::Exact { precision: whole.saturating_add(scale).min(MAX_EXACT_PRECISION), scale },
        (0, false) => DataType::Approximate { precision: DOUBLE_PRECISION },
        _ => DataType::Character { length },
    })
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §6.9] A `<case expression>`: NULLIF (V1, V2) is
    /// CASE WHEN V1 = V2 THEN NULL ELSE V1 END [SR1]; COALESCE (V1, V2,
    /// ...) is CASE WHEN V1 IS NOT NULL THEN V1 ELSE COALESCE (V2, ...) END
    /// [SR2, SR3]; a simple CASE is a searched one whose conditions are
    /// "CO = WO" [SR4b].
    pub(super) fn case(&self, case: &CaseExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let mut whens: Vec<(Cond, Option<&ValueExpression>)> = Vec::new();
        // The type a `?` result takes [ISO/IEC 9075:1992 §17.6 GR5j, l]:
        // none else, as no rule gives it one.
        let mut taken = None;
        let otherwise = match case {
            CaseExpression::NullIf(a, b) => {
                // [§17.6 GR5l] a `?` takes the other operand's type, as
                // NULLIF's result too.
                let (condition, b_type) = self.equal(a, b, None, scope)?;
                taken = Some(b_type);
                whens.push((condition, None));
                Some(a)
            }
            CaseExpression::Coalesce(values) => {
                let (last, rest) = values.split_last().ok_or_else(|| rule("[ISO/IEC 9075:1992 §6.9 F] COALESCE has two values or more"))?;
                // [§17.6 GR5j] a `?` takes the first operand's type; the
                // first none [GR4h].
                let first = values.first().map(|v| self.value_expression(v, scope)).transpose()?.map(|(_, t)| t);
                taken = first;
                for v in rest {
                    let (e, _) = self.typed_by(v, first, scope)?;
                    whens.push((Cond::IsNull(e, true), Some(v)));
                }
                Some(last)
            }
            CaseExpression::Simple { operand, whens: simple, otherwise } => {
                // [§17.6 GR5k] a `?` <when operand> takes the first one's
                // type; the first none [GR4h].
                let first = simple.first().map(|(v, _)| self.value_expression(v, scope)).transpose()?.map(|(_, t)| t);
                for (value, result) in simple {
                    whens.push((self.equal(operand, value, first, scope)?.0, result.as_ref()));
                }
                otherwise.as_ref()
            }
            CaseExpression::Searched { whens: searched, otherwise } => {
                for (condition, result) in searched {
                    whens.push((self.search_condition(condition, scope)?, result.as_ref()));
                }
                otherwise.as_ref()
            }
        };
        self.results(whens, (otherwise, taken), scope)
    }

    /// `v` bound, a `?` taking the type `taken`, if any [§17.6 GR5j, k, l].
    fn typed_by(&self, v: &ValueExpression, taken: Option<DataType>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match taken {
            Some(t) => self.typed(v, ParameterType::Of(t), scope),
            None => self.value_expression(v, scope),
        }
    }

    /// [§6.9 SR4a] "a = b", the two comparable, and b's type: a `?` b taking
    /// the type `b_taken` if given [§17.6 GR5k]; else of the two, a `?`
    /// taking the other's \[GR5l, m\].
    fn equal(&self, a: &ValueExpression, b: &ValueExpression, b_taken: Option<DataType>, scope: Scope<'_>) -> Result<(Cond, DataType)> {
        let [(x, xt), (y, yt)] = match (b_taken, dynamic_of(b)) {
            (Some(t), Some(_)) => {
                let y = self.typed(b, ParameterType::Of(t), scope)?;
                [self.typed(a, ParameterType::Of(y.1), scope)?, y]
            }
            (Some(_), None) | (None, _) => self.pair((a, b), scope)?,
        };
        if !xt.comparable(&yt) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.9 SR4a, §8.2 SR3] {xt} and {yt} are not comparable")));
        }
        Ok((Cond::Compare(x, CompOp::Eq, y), yt))
    }

    /// [§6.9 SR5-SR7, GR2] The CASE of conditions `whens`, each with its
    /// result, NULL if none, and the ELSE result `otherwise`, NULL if none:
    /// one result at least not NULL, the type theirs [§9.3], each cast to it.
    fn results(
        &self,
        whens: Vec<(Cond, Option<&ValueExpression>)>,
        (otherwise, taken): (Option<&ValueExpression>, Option<DataType>),
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let mut bound = Vec::new();
        for result in whens.iter().map(|(_, r)| *r).chain(std::iter::once(otherwise)) {
            bound.push(result.map(|v| self.typed_by(v, taken, scope)).transpose()?);
        }
        let types: Vec<DataType> = bound.iter().flatten().map(|(_, t)| *t).collect();
        if types.is_empty() {
            return Err(rule("[ISO/IEC 9075:1992 §6.9 SR5] at least one result of a CASE is not NULL"));
        }
        let t = set_type(&types, "§6.9 SR7")?;
        let mut results = bound.into_iter().map(|r| match r {
            Some((e, et)) => {
                if et == t {
                    e
                } else {
                    Expr::Cast(Box::new(e), t)
                }
            }
            None => Expr::Const(Value::Null),
        });
        let arms: Vec<(Cond, Expr)> = whens.into_iter().map(|(c, _)| (c, results.next().unwrap_or(Expr::Const(Value::Null)))).collect();
        let last = results.next().unwrap_or(Expr::Const(Value::Null));
        Ok((Expr::Case(arms, Box::new(last)), t))
    }

    /// [ISO/IEC 9075:1992 §6.10] CAST (`operand` AS `target`): NULL, or a
    /// value of any of these types, cast to any [SR4].
    pub(super) fn cast(&self, operand: Option<&ValueExpression>, named: &TypeName, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        // [§6.10 SR1, AR1] a domain's data type, if USAGE on it is held.
        let target = match named {
            TypeName::Data(t) => *t,
            TypeName::Domain(name) => self.db.usable_domain(&super::qualify(name, self.schema), self.auth).or_at(name.at)?.data_type,
        };
        let Some(v) = operand else { return Ok((Expr::Const(Value::Null), target)) };
        // [§17.6 GR5i] a `?` takes the target's type.
        let (e, _) = self.typed(v, ParameterType::Of(target), scope)?;
        Ok((Expr::Cast(Box::new(e), target), target))
    }

    /// [ISO/IEC 9075:1992 §7.11] A `<scalar subquery>`: of degree one [SR1].
    pub(super) fn scalar_subquery(&self, s: &Subquery, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (bound, types) = self.subquery(s, scope, Use::Rows)?;
        let [t] = types.as_slice() else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §7.11 SR1] a <scalar subquery> is of degree one, not {}", types.len())));
        };
        Ok((Expr::Subquery(bound, 0), *t))
    }
}
