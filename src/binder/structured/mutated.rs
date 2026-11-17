//! [ISO/IEC 9075-2:2023 §14.15 SR7, SR9] A `<mutated set clause>`
//! `OC.M1.M2 ... Mn = RCVE`, bound as the `<set clause>` it is equivalent
//! to, `OC = OC.M1(OC.M1.M2( ... OC.M1 ... Mn-1.Mn(RCVE)))`: each
//! attribute's mutator applied to the value of the one before, observed
//! by its observers. Of a column an earlier mutated set clause sets, its
//! value is that clause's, in place of the column's, and the earlier
//! clause is deleted [SR7c ii].

use super::super::{Binder, Scope, rule};
use super::call;
use crate::ast::{SetClause, UpdateSource};
use crate::error::Result;
use crate::functions::{Mutator, Observer};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::DataType;

/// `e` with each reference to column `position` of the row changed `by`.
fn substituted(e: Expr, position: usize, by: &Expr) -> Expr {
    if matches!(e, Expr::Column(p) if p == position) {
        return by.clone();
    }
    e.map_children(&mut |c| substituted(c, position, by))
}

/// An attribute on the path of a mutated set clause: its subject's value
/// and declared type, and its position, name and declared type.
struct Step {
    subject: Expr,
    of: DataType,
    index: usize,
    name: String,
    attribute: DataType,
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §14.15 SR7, SR9] The value of the mutated set
    /// clause `sc` of the column at `position` of declared type `t`, whose
    /// value is `current`: the column's, or an earlier mutated set
    /// clause's. Its update source contains no set function (`set_function`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column, or an attribute before the last, is of
    /// no structured type, an attribute is not there, or the update source
    /// is not assignable to the last [§9.18 SR10l iii]; -104 if EXECUTE is
    /// not held on an observer or mutator [§9.18 SR6].
    pub(in crate::binder) fn mutated_value(
        &self,
        (sc, position, t): (&SetClause, usize, &DataType),
        current: &Expr,
        set_function: &'static str,
    ) -> Result<Expr> {
        let mut steps: Vec<Step> = Vec::with_capacity(sc.attributes.len());
        let (mut subject, mut of) = (current.clone(), t.clone());
        for name in &sc.attributes {
            let DataType::Structured(s) = &of else {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §14.15 SR7b iv, §9.19 SR3] attribute {name} is set of a value of {of}, no structured type"
                );
                return Err(rule(msg));
            };
            let (index, attribute) = self.mutator_of(&s.name, name)?;
            steps.push(Step { subject: subject.clone(), of: of.clone(), index, name: name.clone(), attribute: attribute.clone() });
            // [§9.18 SR6] the observer of each attribute but the last.
            if steps.len() < sc.attributes.len() {
                self.observer_held(&s.name, name)?;
                subject = call(Observer { name: name.clone(), index, result: attribute.clone() }, vec![subject]);
            }
            of = attribute;
        }
        let last = steps.last().ok_or_else(|| rule("[ISO/IEC 9075-2:2023 §14.15] a mutated set clause names an attribute"))?;
        let mut value = match &sc.value {
            UpdateSource::Null => Expr::Const(crate::value::Value::Null),
            UpdateSource::Default => {
                let DataType::Structured(s) = &last.of else { return Err(rule("[DESIGN] an attribute's subject is structured")) };
                let desc = self.structured_desc(&s.name)?;
                let (_, a) = desc.attribute(&last.name).ok_or_else(|| rule("[DESIGN] the attribute is there"))?;
                self.attribute_default(a)?
            }
            UpdateSource::Value(v) => {
                let (e, given) = self.typed(v, ParameterType::Of(last.attribute.clone()), Scope::Rows(set_function))?;
                let Some(e) = self.assigned((e, &given), &last.attribute)? else {
                    let msg = format!(
                        "[ISO/IEC 9075-2:2023 §9.18 SR10l iii] attribute {} is of {}, which a value of {given} cannot be stored in",
                        last.name, last.attribute
                    );
                    return Err(rule(msg));
                };
                substituted(e, position, current)
            }
        };
        for step in steps.into_iter().rev() {
            let mutator = Mutator { name: step.name, index: step.index, attribute: step.attribute, result: step.of };
            value = call(mutator, vec![step.subject, value]);
        }
        Ok(value)
    }
}
