//! Identity-constraint definitions read from `<key>`, `<keyref>` and
//! `<unique>` [Schema1 §3.11.2], their selector and fields parsed as the
//! restricted XPath of [Schema1 §3.11.6], and each keyref's referenced key
//! resolved once all are read.

use super::super::component::{ConstraintId, Expanded, IdentityCategory, IdentityConstraint};
use super::super::{invalid, xpath};
use super::attrs::At;
use super::build::{Builder, Place};
use super::element::annotation;
use crate::error::Result;

impl Builder<'_> {
    /// The identity-constraint definition `<key>`, `<keyref>` or `<unique>`
    /// `place` represents [Schema1 §3.11.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has not one `<selector>` and one or more
    /// `<field>`s, a selector or field is not of the XPath subset [Schema1
    /// §3.11.6 c-selector-xpath, c-fields-xpaths], or a keyref has no
    /// refer.
    pub fn read_constraint(&mut self, place: Place) -> Result<ConstraintId> {
        if let Some(&id) = self.constraints.get(&place) {
            return Ok(id);
        }
        let at = self.at(place)?;
        let tns = self.doc(place.0)?.tns.clone();
        let category = match at.xs_name() {
            Some("key") => IdentityCategory::Key,
            Some("keyref") => IdentityCategory::KeyRef,
            _ => IdentityCategory::Unique,
        };
        let name = Expanded::new(tns.as_deref(), &at.ncname("name")?.unwrap_or_default());
        let children = at.children()?;
        let selectors: Vec<&At<'_>> = children.iter().filter(|(_, l)| *l == "selector").map(|(a, _)| a).collect();
        let [selector] = selectors.as_slice() else {
            return Err(invalid("[Schema1 §3.11.2]", "an identity-constraint definition has one <selector>"));
        };
        let selector = paths(selector, false, "[Schema1 §3.11.6 c-selector-xpath]")?;
        let mut fields = Vec::new();
        for (field, _) in children.iter().filter(|(_, l)| *l == "field") {
            fields.push(paths(field, true, "[Schema1 §3.11.6 c-fields-xpaths]")?);
        }
        if fields.is_empty() {
            return Err(invalid("[Schema1 §3.11.2]", "an identity-constraint definition has a <field>"));
        }
        let id = ConstraintId(self.schema.constraints.len());
        if category == IdentityCategory::KeyRef {
            let refer = self.qname(place.0, &at, "refer")?.ok_or_else(|| invalid("[Schema1 §3.11.2]", "a <keyref> has a refer"))?;
            self.refers.push((id, refer));
        }
        let annotation = annotation(place.0, &at)?;
        self.schema.constraints.push(IdentityConstraint { name, category, selector, fields, refer: None, annotation });
        self.constraints.insert(place, id);
        Ok(id)
    }

    /// Each keyref's {referenced key} resolved [Schema1 §3.11.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its refer names no key or unique, or the key has
    /// as many fields as it [Schema1 §3.11.6 c-props-correct.2].
    pub fn finish_refers(&mut self) -> Result<()> {
        for (id, name) in std::mem::take(&mut self.refers) {
            let refer = self.schema.constraint_names.get(&name).copied().or_else(|| {
                let place = self.index.constraints.get(&name)?;
                self.constraints.get(place).copied()
            });
            let key = refer.and_then(|r| self.schema.constraint(r)).filter(|k| k.category != IdentityCategory::KeyRef);
            let Some(key) = key else { return Err(Builder::unresolved(&name, "key or unique")) };
            let fields = key.fields.len();
            let Some(keyref) = self.schema.constraints.get_mut(id.0) else { continue };
            if keyref.fields.len() != fields {
                return Err(invalid("[Schema1 §3.11.6 c-props-correct.2]", "a keyref has as many fields as the key it refers to"));
            }
            keyref.refer = refer;
        }
        Ok(())
    }
}

/// The paths of the xpath attribute of `<selector>` or `<field>` `at`, a
/// field's if `field`.
///
/// # Errors
///
/// SQLSTATE 22000, citing `rule`, if it has none, or one not of the
/// subset.
fn paths(at: &At<'_>, field: bool, rule: &str) -> Result<Vec<xpath::Path>> {
    let text = at.attr("xpath").ok_or_else(|| invalid("[Schema1 §3.11.2]", &format!("<{}> has an xpath", at.local())))?;
    xpath::parse(text, field, &at.scope()).ok_or_else(|| invalid(rule, &format!("'{text}' is no expression of the XPath subset")))
}
