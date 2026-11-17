//! [ISO/IEC 9075-2:2023 §14.15 SR7, SR9] The `<set clause list>` of an
//! UPDATE of a view an INSTEAD OF trigger takes, each `<mutated set
//! clause>` `OC.M1 ... Mn = RCVE` replaced by the `<set clause>` it is
//! equivalent to, `OC = OC.M1(OC.M1.M2( ... Mn(RCVE)))`, its methods those
//! of the view's column's type. NULL and DEFAULT are typed by the last
//! attribute [SR9; §6.5 SR3]. Of a column an earlier mutated set clause
//! sets, the value is that clause's in place of the column's, and the
//! earlier clause is deleted [SR7c ii].

use super::super::{State, rule};
use crate::ast::{ColumnSpecification, SetClause, StructuredExpression, TypeName, UpdateSource, ValueExpression};
use crate::catalog::{AttributeDesc, ViewDesc, udt_qualified};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;
use crate::value::DataType;

/// The method invocation `subject`.`name`(`args`), written at `at`.
fn method(subject: ValueExpression, name: &str, args: Vec<ValueExpression>, at: usize) -> ValueExpression {
    let m = StructuredExpression::Method { subject, as_type: None, name: name.to_owned(), args };
    ValueExpression::Structured(Box::new(m), at)
}

/// CAST (`v` AS `t`), or of NULL if `v` is none.
fn cast(v: Option<ValueExpression>, t: &DataType, at: usize) -> ValueExpression {
    ValueExpression::Cast { operand: v.map(Box::new), target: TypeName::Data(t.clone()), format: None, passing: None, at }
}

/// Of the structured type `t` in `st`, its attribute `name`.
fn attribute(st: &State, t: &DataType, name: &str) -> Result<AttributeDesc> {
    let DataType::Structured(s) = t else {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §14.15 SR7b iv, §9.19 SR3] attribute {name} is set of a value of {t}, no structured type"
        )));
    };
    let desc = st
        .catalog
        .structured_type(&udt_qualified(&s.name))
        .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §14.15 SR7] there is no type {}", s.name)))?;
    let (_, a) =
        desc.attribute(name).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] type {} has no attribute {name}", desc.name)))?;
    Ok(a.clone())
}

/// The mutated set clause `sc` of the view's column of type `t`, whose
/// value is `current`, as the value of the set clause it is equivalent to.
fn equivalent(st: &State, (sc, t): (&SetClause, &DataType), current: ValueExpression) -> Result<ValueExpression> {
    let mut subjects = vec![current];
    let mut of = t.clone();
    let mut last = None;
    for (k, name) in sc.attributes.iter().enumerate() {
        let a = attribute(st, &of, name)?;
        if k.saturating_add(1) < sc.attributes.len() {
            let before = subjects.last().cloned().ok_or_else(|| rule("[DESIGN] a subject"))?;
            subjects.push(method(before, name, Vec::new(), sc.at));
        }
        of = a.data_type.clone();
        last = Some(a);
    }
    let last = last.ok_or_else(|| rule("[ISO/IEC 9075-2:2023 §14.15] a mutated set clause names an attribute"))?;
    let mut value = match &sc.value {
        UpdateSource::Value(v) => v.clone(),
        UpdateSource::Null => cast(None, &last.data_type, sc.at),
        UpdateSource::Default => match &last.default {
            Some(text) => cast(Some(Parser::new(text)?.of_definition().generation_expression()?), &last.data_type, sc.at),
            None => cast(None, &last.data_type, sc.at),
        },
    };
    for (name, subject) in sc.attributes.iter().zip(subjects).rev() {
        value = method(subject, name, vec![value], sc.at);
    }
    Ok(value)
}

/// [ISO/IEC 9075-2:2023 §14.15 SR7] `set`, an UPDATE's of `view` in `st`,
/// each mutated set clause replaced by the set clause it is equivalent to.
///
/// # Errors
///
/// SQLCODE -103 if a column or an attribute before the last is of no
/// structured type, or an attribute is not there; -900 of an update source
/// of a later mutated set clause of a column that reads the column, whose
/// value would be the earlier clause's (RULES.md D81).
pub(super) fn unmutated(st: &State, view: &ViewDesc, set: &[SetClause]) -> Result<Vec<SetClause>> {
    let mut out: Vec<SetClause> = Vec::with_capacity(set.len());
    let mut mutated: Vec<String> = Vec::new();
    for sc in set {
        if sc.attributes.is_empty() {
            out.push(sc.clone());
            continue;
        }
        let t = view.columns.iter().find(|c| c.name == sc.column).map(|c| c.data_type.clone());
        let t = t.ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §14.11 SR10] view {} has no column {}", view.name, sc.column)))?;
        let column = ValueExpression::Column(ColumnSpecification { qualifier: None, column: sc.column.clone(), catalog: None, at: sc.at });
        let earlier = mutated.contains(&sc.column).then(|| out.iter().position(|o| o.column == sc.column)).flatten();
        let current = match earlier.map(|k| out.remove(k)) {
            Some(SetClause { value: UpdateSource::Value(v), .. }) => {
                let reads = |w: &ValueExpression| w.any(&|e| matches!(e, ValueExpression::Column(c) if c.column == sc.column));
                if let UpdateSource::Value(source) = &sc.value
                    && reads(source)
                {
                    let msg = format!(
                        "[DESIGN] a later mutated set clause of column {} of a view an INSTEAD OF trigger takes reads the column, which is valid SQL this implementation does not take (RULES.md D81)",
                        sc.column
                    );
                    return Err(SqlError::new(Code::Unsupported, msg));
                }
                v
            }
            Some(_) | None => column,
        };
        let value = equivalent(st, (sc, &t), current)?;
        out.push(SetClause {
            column: sc.column.clone(),
            attributes: Vec::new(),
            element: None,
            value: UpdateSource::Value(value),
            at: sc.at,
        });
        mutated.push(sc.column.clone());
    }
    Ok(out)
}
