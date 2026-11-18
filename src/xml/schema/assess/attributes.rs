//! The attributes of an element assessed [Schema1 §3.4.4 clauses 3 to 5]:
//! each by the attribute use of its name, else by the type's attribute
//! wildcard and its {process contents}; the required ones present; the
//! defaults of those absent added [Schema1 §3.4.5]; and the four
//! attributes of the XML Schema instance namespace by their built-in
//! declarations [Schema1 §3.2.7].

use super::super::builtin::{is_id, string_valid};
use super::super::component::{AttributeDecl, AttributeUse, ComplexId, Process, Simple, TypeDef, Typed, ValueKind, Wildcard};
use super::element::{XSI_NIL, XSI_TYPE, annotation, expanded};
use super::run::{Assess, Run};
use super::{Failure, Step};
use crate::xml::{Annotation, AtomicType, NodeId, QName, Specified, XSI_NAMESPACE};

/// [Schema1 §3.2.7] The local name of `xsi:schemaLocation`.
const SCHEMA_LOCATION: &str = "schemaLocation";
/// [Schema1 §3.2.7] The local name of `xsi:noNamespaceSchemaLocation`.
const NO_NAMESPACE_SCHEMA_LOCATION: &str = "noNamespaceSchemaLocation";

/// The attributes of an element made: each as specified, and the
/// annotation of each, none of one not assessed.
#[derive(Debug, Default)]
pub(super) struct Attrs {
    /// The attributes, their values normalized.
    pub specified: Vec<Specified>,
    /// Each one's annotation.
    pub annotations: Vec<Option<Annotation>>,
}

impl Attrs {
    /// `name` of value `value`, of annotation `annotation`, added.
    fn push(&mut self, name: QName, value: String, annotation: Option<Annotation>) {
        self.specified.push(Specified { name, value, is_id: false, is_idrefs: false });
        self.annotations.push(annotation);
    }
}

/// The type of a built-in attribute of the XML Schema instance namespace
/// named `local`, if it is one of the four [Schema1 §3.2.7].
fn instance_type(local: &str) -> Option<Simple> {
    match local {
        XSI_TYPE => Some(Simple::Atomic(AtomicType::QName)),
        XSI_NIL => Some(Simple::Atomic(AtomicType::Boolean)),
        SCHEMA_LOCATION => Some(Simple::List(AtomicType::AnyUri)),
        NO_NAMESPACE_SCHEMA_LOCATION => Some(Simple::Atomic(AtomicType::AnyUri)),
        _ => None,
    }
}

impl Run<'_> {
    /// The attribute `a` of element `element` if it is one of the four of
    /// the XML Schema instance namespace, read by its built-in declaration
    /// [Schema1 §3.2.7].
    ///
    /// # Errors
    ///
    /// A failure if its value is not valid for it [Schema1 §3.2.4
    /// cvc-attribute.3].
    fn instance(&self, element: NodeId, a: NodeId, out: &mut Attrs) -> Step<bool> {
        let Some(name) = self.tree.name(a).filter(|n| n.uri.as_deref() == Some(XSI_NAMESPACE)) else { return Ok(false) };
        let Some(s) = instance_type(&name.local) else { return Ok(false) };
        let value = self.read(s, self.tree.content(a), element)?;
        out.push(name.clone(), value.normalized.to_string(), Some(annotation(self.schema, TypeDef::Simple(s))));
        Ok(true)
    }

    /// `text` read against `s`, QNames by the namespaces in scope of
    /// `element` [Schema1 §3.14.4].
    fn read(&self, s: Simple, text: &str, element: NodeId) -> Step<Typed> {
        let read = string_valid(s, text, &self.tree.in_scope(element))?;
        read.ok_or_else(|| Failure::new("[Schema1 §3.2.4 cvc-attribute.3]", format!("'{text}' is no value of the attribute's type")).into())
    }

    /// [Schema1 §3.3.4 cvc-type.3.1.1] The attributes of `element`, of a
    /// simple type: those of the XML Schema instance namespace alone.
    pub(super) fn simple_attributes(&mut self, element: NodeId) -> Step<Attrs> {
        let mut out = Attrs::default();
        for a in self.tree.attributes(element) {
            if !self.instance(element, a, &mut out)? {
                let name = expanded(self.tree, a);
                return Err(Failure::new(
                    "[Schema1 §3.3.4 cvc-type.3.1.1]",
                    format!("an element of a simple type has no attribute {}", name.text()),
                )
                .into());
            }
        }
        Ok(out)
    }

    /// [Schema1 §3.4.4 cvc-complex-type.3, 4, 5] [Schema1 §3.4.5] The
    /// attributes of `element`, of complex type `c`.
    pub(super) fn complex_attributes(&mut self, element: NodeId, c: ComplexId) -> Step<Attrs> {
        let Some(def) = self.schema.complex(c) else { return Ok(Attrs::default()) };
        let mut out = Attrs::default();
        let mut present = vec![false; def.attribute_uses.len()];
        let mut wild_ids = 0usize;
        for a in self.tree.attributes(element) {
            if self.instance(element, a, &mut out)? {
                continue;
            }
            let name = expanded(self.tree, a);
            let used = def.attribute_uses.iter().position(|u| self.schema.attribute(u.decl).is_some_and(|d| d.name == name));
            match used.and_then(|i| def.attribute_uses.get(i).map(|u| (i, u))) {
                Some((i, u)) => {
                    if let Some(p) = present.get_mut(i) {
                        *p = true;
                    }
                    self.by_use(element, a, u, &mut out)?;
                }
                None => {
                    if self.by_wildcard(element, a, def.wildcard.as_ref(), &mut out)? {
                        wild_ids = wild_ids.saturating_add(1);
                    }
                }
            }
        }
        self.wild_ids(wild_ids, &def.attribute_uses)?;
        self.required_and_defaults(&def.attribute_uses, &present, &mut out, element)?;
        Ok(out)
    }

    /// [Schema1 §3.4.4 cvc-complex-type.3.1] Attribute `a` of `element`
    /// by attribute use `u` [Schema1 §3.2.4 cvc-attribute, §3.5.4 cvc-au].
    fn by_use(&mut self, element: NodeId, a: NodeId, u: &AttributeUse, out: &mut Attrs) -> Step<()> {
        let Some(decl) = self.schema.attribute(u.decl) else { return Ok(()) };
        self.by_attribute_declaration(element, a, decl, out)?;
        let value = self.read(decl.type_def, self.tree.content(a), element)?;
        if let Some(fixed) = u.value.as_ref().filter(|v| v.kind == ValueKind::Fixed)
            && !value.equal(&fixed.value)
        {
            return Err(Failure::new("[Schema1 §3.5.4 cvc-au]", format!("{} is not its fixed value", decl.name.text())).into());
        }
        Ok(())
    }

    /// [Schema1 §3.2.4 cvc-attribute] Attribute `a` of `element` by
    /// declaration `decl`: its normalized value valid, its fixed value
    /// matched; its IDs and IDREFs noted [Schema1 §3.15.5].
    fn by_attribute_declaration(&mut self, element: NodeId, a: NodeId, decl: &AttributeDecl, out: &mut Attrs) -> Step<()> {
        let value = self.read(decl.type_def, self.tree.content(a), element)?;
        if let Some(fixed) = decl.value.as_ref().filter(|v| v.kind == ValueKind::Fixed)
            && !value.equal(&fixed.value)
        {
            return Err(Failure::new("[Schema1 §3.2.4 cvc-attribute.4]", format!("{} is not its fixed value", decl.name.text())).into());
        }
        self.note_ids(decl.type_def, &value);
        let name = self.tree.name(a).cloned().unwrap_or_else(|| QName::local(""));
        out.push(name, value.normalized.to_string(), Some(annotation(self.schema, TypeDef::Simple(decl.type_def))));
        Ok(())
    }

    /// [Schema1 §3.4.4 cvc-complex-type.3.2] Attribute `a` of `element`,
    /// which no attribute use names, by `wildcard` [Schema1 §3.10.4]:
    /// whether it was validated by a declaration of an ID type, a wild ID
    /// \[5\].
    fn by_wildcard(&mut self, element: NodeId, a: NodeId, wildcard: Option<&Wildcard>, out: &mut Attrs) -> Step<bool> {
        let name = expanded(self.tree, a);
        let Some(w) = wildcard else {
            return Err(
                Failure::new("[Schema1 §3.4.4 cvc-complex-type.3.2.1]", format!("the element has no attribute {}", name.text())).into()
            );
        };
        if !w.constraint.allows(name.uri.as_deref()) {
            return Err(
                Failure::new("[Schema1 §3.4.4 cvc-complex-type.3.2.2]", format!("no wildcard allows attribute {}", name.text())).into()
            );
        }
        let how = match w.process {
            Process::Skip => Assess::Skip,
            Process::Lax => Assess::Lax,
            Process::Strict => Assess::Strict,
        };
        self.other_attribute(element, a, how, out)
    }

    /// Attribute `a` of `element` assessed as `how` says [Schema1 §3.10.4,
    /// §3.2.4 cvc-assess-attr]: by its top-level declaration if it has one
    /// and is not skipped, which strict assessment requires; else kept as
    /// it is, not assessed. Whether its declaration is of an ID type.
    fn other_attribute(&mut self, element: NodeId, a: NodeId, how: Assess, out: &mut Attrs) -> Step<bool> {
        let name = expanded(self.tree, a);
        let found = self.schema.attribute_names.get(&name).and_then(|&d| self.schema.attribute(d)).filter(|_| how != Assess::Skip);
        if let Some(decl) = found {
            self.by_attribute_declaration(element, a, decl, out)?;
            return Ok(is_id(decl.type_def));
        }
        if how == Assess::Strict {
            return Err(Failure::new("[Schema1 §3.2.4 cvc-assess-attr.1]", format!("attribute {} has no declaration", name.text())).into());
        }
        let qname = self.tree.name(a).cloned().unwrap_or_else(|| QName::local(""));
        out.push(qname, self.tree.content(a).to_owned(), None);
        Ok(false)
    }

    /// The attributes of `element` not assessed, if `how` is skip, or
    /// laxly assessed by the ur-type's attribute wildcard [Schema1
    /// §3.4.7].
    pub(super) fn other_attributes(&mut self, element: NodeId, how: Assess) -> Step<Attrs> {
        let mut out = Attrs::default();
        for a in self.tree.attributes(element) {
            if how != Assess::Skip && self.instance(element, a, &mut out)? {
                continue;
            }
            self.other_attribute(element, a, how, &mut out)?;
        }
        Ok(out)
    }

    /// [Schema1 §3.4.4 cvc-complex-type.5] At most one wild ID, and none
    /// if `uses` has an attribute of an ID type.
    fn wild_ids(&self, count: usize, uses: &[AttributeUse]) -> Step<()> {
        if count > 1 {
            return Err(Failure::new("[Schema1 §3.4.4 cvc-complex-type.5.1]", "an element has at most one attribute of an ID type").into());
        }
        let declared_id = uses.iter().filter_map(|u| self.schema.attribute(u.decl)).any(|d| is_id(d.type_def));
        if count == 1 && declared_id {
            return Err(Failure::new("[Schema1 §3.4.4 cvc-complex-type.5.2]", "an element has at most one attribute of an ID type").into());
        }
        Ok(())
    }

    /// [Schema1 §3.4.4 cvc-complex-type.4] Each required use of `uses`
    /// present; [Schema1 §3.4.5] each absent use with a value constraint,
    /// its own or its declaration's, added with its canonical value.
    fn required_and_defaults(&mut self, uses: &[AttributeUse], present: &[bool], out: &mut Attrs, element: NodeId) -> Step<()> {
        for (u, &here) in uses.iter().zip(present) {
            let Some(decl) = self.schema.attribute(u.decl) else { continue };
            if here {
                continue;
            }
            if u.required {
                return Err(Failure::new(
                    "[Schema1 §3.4.4 cvc-complex-type.4]",
                    format!("the element has no attribute {}", decl.name.text()),
                )
                .into());
            }
            let Some(value) = u.value.as_ref().or(decl.value.as_ref()) else { continue };
            let canonical = value.value.canonical();
            let read = self.read(decl.type_def, &canonical, element)?;
            self.note_ids(decl.type_def, &read);
            let name = QName {
                prefix: self.prefix_of(element, decl.name.uri.as_deref()),
                uri: decl.name.uri.clone(),
                local: decl.name.local.clone(),
            };
            out.push(name, read.normalized.to_string(), Some(annotation(self.schema, TypeDef::Simple(decl.type_def))));
        }
        Ok(())
    }

    /// A prefix bound to `uri` in scope of `element`, for an attribute
    /// added in that namespace; none of no namespace.
    fn prefix_of(&self, element: NodeId, uri: Option<&str>) -> Option<Box<str>> {
        let uri = uri?;
        self.tree.in_scope(element).into_iter().find(|(p, u)| p.is_some() && *u == uri).and_then(|(p, _)| p.map(Into::into))
    }
}
