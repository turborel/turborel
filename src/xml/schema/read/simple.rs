//! Simple type definitions read from `<simpleType>` [Schema1 §3.14.2]
//! [Schema2 §4.1.2]: derived by restriction with facets [Schema2 §4.3],
//! by list or by union; the constraints on their representations [Schema1
//! §3.14.3] and on their bases' and item and member types' {final}
//! [Schema1 §3.14.6]; and the simple type a restriction of simple content
//! derives [Schema1 §3.4.2].
//!
//! \[DESIGN\] A simple type definition is read whole when first named, as
//! it names no type but simple ones, so one named while it is being read
//! is a circular definition. Its datatype is made by `SimpleType`'s
//! `restrict`, `list` and `union`, which keep the rules of the facets
//! themselves [Schema2 §4.1.5, §4.3]: Derivation Valid (Restriction,
//! Simple) [Schema1 §3.14.6 cos-st-restricts] is those rules and the
//! {final} checked here. Reading recurses through the types a definition
//! names, so the functions it passes through hold a datatype only behind
//! an `Arc`, made by functions it does not pass through, which keeps each
//! level's frames small.

use std::sync::Arc;

use super::super::builtin::simple_type;
use super::super::component::{Derivations, Expanded, Simple, SimpleId, SimpleTypeDef, TypeDef};
use super::super::{Facet, FacetSpec, MAX_CHAIN, SimpleType, Variety, beyond, invalid};
use super::attrs::At;
use super::build::{Builder, Place, State};
use super::element::annotation;
use super::index::Space;
use crate::error::Result;
use crate::xml::{AtomicType, WhiteSpace, from_lexical};

/// [Schema2 §4.1.2.1] The children of `<restriction>` that are facets.
pub(super) const FACETS: [&str; 12] = [
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "totalDigits",
    "fractionDigits",
    "length",
    "minLength",
    "maxLength",
    "enumeration",
    "whiteSpace",
    "pattern",
];

/// What a `<restriction>`, `<list>` or `<union>` derives: the {base type
/// definition}, the {member type definitions} and the datatype.
type Derived = (Simple, Vec<Simple>, Arc<SimpleType>);

/// The error of a redefinition of a simple type that does not restrict
/// the type it redefines [Schema1 §4.2.2 src-redefine.5].
fn not_self_restriction() -> crate::error::SqlError {
    invalid("[Schema1 §4.2.2 src-redefine.5]", "a redefined simple type restricts the type it redefines")
}

impl Builder<'_> {
    /// The simple type definition `<simpleType>` `place` represents, read
    /// if new [Schema1 §3.14.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it is needed while being read, a circular
    /// definition [Schema1 §3.14.6 st-props-correct.2]; as `read_simple`.
    pub fn simple_at(&mut self, place: Place) -> Result<SimpleId> {
        let id = if let Some(&id) = self.simple.get(&place) {
            id
        } else {
            let id = SimpleId(self.schema.simple_types.len());
            self.schema.simple_types.push(placeholder());
            self.simple_state.push(State::Pending);
            self.simple.insert(place, id);
            id
        };
        match self.simple_state.get(id.0).copied() {
            Some(State::Done) | None => Ok(id),
            Some(State::Filling) => {
                Err(invalid("[Schema1 §3.14.6 st-props-correct.2]", "a simple type definition is defined by way of itself"))
            }
            Some(State::Pending) => {
                self.fill_simple(id, place)?;
                Ok(id)
            }
        }
    }

    /// Simple type `id`, at `place`, read, within the simple types being
    /// read.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `MAX_CHAIN` of them; as `read_simple`.
    fn fill_simple(&mut self, id: SimpleId, place: Place) -> Result<()> {
        self.set_simple_state(id, State::Filling);
        let depth = self.simple_depth.saturating_add(1);
        if depth > MAX_CHAIN {
            return Err(beyond(&format!("its simple type definitions are defined by way of more than {MAX_CHAIN} others")));
        }
        let outer = std::mem::replace(&mut self.simple_depth, depth);
        let anonymous = self.originals.contains(&place);
        let def = self.in_context(place, |b| b.read_simple(place, anonymous));
        self.simple_depth = outer;
        let def = def?;
        if let Some(slot) = self.schema.simple_types.get_mut(id.0) {
            *slot = def;
        }
        self.set_simple_state(id, State::Done);
        Ok(())
    }

    /// Simple type `id`'s state set to `state`.
    fn set_simple_state(&mut self, id: SimpleId, state: State) {
        if let Some(s) = self.simple_state.get_mut(id.0) {
            *s = state;
        }
    }

    /// The simple type definition `<simpleType>` `place` represents
    /// [Schema1 §3.14.2]; anonymous if `anonymous`, as a definition a
    /// `<redefine>` replaces is [Schema1 §4.2.2 clause 1.1].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has not one of `<restriction>`, `<list>` and
    /// `<union>`, or it redefines a type it does not restrict [Schema1
    /// §4.2.2 src-redefine.5]; as the reading of its derivation.
    fn read_simple(&mut self, place: Place, anonymous: bool) -> Result<SimpleTypeDef> {
        let at = self.at(place)?;
        let doc = self.doc(place.0)?;
        let name = if anonymous { None } else { at.ncname("name")?.map(|n| Expanded::new(doc.tns.as_deref(), &n)) };
        let listed = Derivations::RESTRICTION.with(Derivations::LIST).with(Derivations::UNION);
        let final_ = at.derivations_all("final", (listed, Derivations::SIMPLE_FINAL), doc.final_default)?.within(Derivations::SIMPLE_FINAL);
        let children: Vec<(At<'_>, &str)> = at.children()?.into_iter().filter(|(_, l)| *l != "annotation").collect();
        let [(derivation, which)] = children.as_slice() else {
            return Err(invalid("[Schema1 §3.14.2]", "a <simpleType> has one <restriction>, <list> or <union>"));
        };
        let own = name.as_ref().filter(|_| self.redefining.as_ref().is_some_and(|r| r.space == Space::Type));
        let (base, members, def) = match (*which, own) {
            ("restriction", _) => self.simple_restriction(derivation, place.0, own)?,
            ("list", None) => self.simple_list(derivation, place.0)?,
            ("union", None) => self.simple_union(derivation, place.0)?,
            (_, Some(_)) => return Err(not_self_restriction()),
            (other, None) => return Err(invalid("[Schema1 §3.14.2]", &format!("<{other}> derives no simple type"))),
        };
        let def = match &name {
            Some(n) => renamed(def, &n.text()),
            None => def,
        };
        Ok(SimpleTypeDef { name, namespace: doc.tns.clone(), base, final_, members, def, annotation: annotation(place.0, &at)? })
    }

    /// What `<restriction>` `at` of document `doc` derives [Schema2
    /// §4.1.2.1]: its base, named or its `<simpleType>`, restricted by its
    /// facets; of a redefinition named `own`, its base named so [Schema1
    /// §4.2.2 src-redefine.5].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has both or neither of a base and a
    /// `<simpleType>` [Schema1 §3.14.3 src-simple-type.2]; as `restricted`.
    fn simple_restriction(&mut self, at: &At<'_>, doc: usize, own: Option<&Expanded>) -> Result<Derived> {
        let children = at.children()?;
        let inner = children.iter().find(|(_, l)| *l == "simpleType").map(|(c, _)| c.node);
        let named = self.qname(doc, at, "base")?;
        if own.is_some() && own != named.as_ref() {
            return Err(not_self_restriction());
        }
        let base = match (named, inner) {
            (Some(name), None) => match self.type_named(&name)? {
                TypeDef::Simple(s) => s,
                TypeDef::Complex(_) => {
                    return Err(invalid("[Schema1 §3.14.2]", &format!("{} is no simple type definition", name.text())));
                }
            },
            (None, Some(node)) => Simple::Defined(self.simple_at((doc, node))?),
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid("[Schema1 §3.14.3 src-simple-type.2]", "a <restriction> has a base or a <simpleType>, not both"));
            }
        };
        let def = self.restricted(base, &facet_specs(&children)?)?;
        Ok((base, self.members_of(base), def))
    }

    /// `base` restricted by `facets` [Schema1 §3.14.3 Simple Type
    /// Restriction (Facets)].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if `base` is the simple ur-type [Schema1 §3.14.1],
    /// `xs:NOTATION` and no enumeration is given [Schema2 §3.2.19], or its
    /// {final} has restriction [Schema1 §3.14.6 st-props-correct.3], or of
    /// a list list, of a union union \[4.2\]; as `SimpleType::restrict`.
    fn restricted(&self, base: Simple, facets: &[FacetSpec]) -> Result<Arc<SimpleType>> {
        let enumerated = facets.iter().any(|f| matches!(f.facet, Facet::Enumeration(_)));
        if base == Simple::Atomic(AtomicType::Notation) && !enumerated {
            return Err(invalid("[Schema2 §3.2.19]", "a type restricting NOTATION specifies an enumeration"));
        }
        let ty = self.datatype(base, "[Schema1 §3.14.1]", "the simple ur-type is the base of no user-defined simple type")?;
        let final_ = self.schema.simple_final(base);
        let (method, rule) = match ty.variety() {
            Variety::Atomic(_) => (Derivations::RESTRICTION, "[Schema1 §3.14.6 st-props-correct.3]"),
            Variety::List(_) => (Derivations::LIST, "[Schema1 §3.14.6 st-props-correct.4.2.1]"),
            Variety::Union(_) => (Derivations::UNION, "[Schema1 §3.14.6 st-props-correct.4.2.2]"),
        };
        if final_.has(Derivations::RESTRICTION) {
            return Err(invalid("[Schema1 §3.14.6 st-props-correct.3]", &format!("{} is final for restriction", ty.name())));
        }
        if method != Derivations::RESTRICTION && final_.has(method) {
            return Err(invalid(rule, &format!("{} is final for its variety", ty.name())));
        }
        SimpleType::restrict(&ty, facets).map(Arc::new)
    }

    /// The datatype of simple type `s`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000, by `rule`, `why`, if it is the simple ur-type,
    /// which has none; as `simple_type`.
    fn datatype(&self, s: Simple, rule: &str, why: &str) -> Result<Arc<SimpleType>> {
        simple_type(&self.schema, s)?.ok_or_else(|| invalid(rule, why))
    }

    /// The {member type definitions} of simple type `s`: none but of a
    /// union.
    fn members_of(&self, s: Simple) -> Vec<Simple> {
        match s {
            Simple::Defined(id) => self.schema.simple(id).map(|d| d.members.clone()).unwrap_or_default(),
            Simple::AnySimple | Simple::Atomic(_) | Simple::List(_) => Vec::new(),
        }
    }

    /// Whether simple type `s` may be used by `method`, list or union: its
    /// {final} has it not [Schema1 §3.14.1].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has.
    fn usable(&self, s: Simple, method: Derivations) -> Result<()> {
        if self.schema.simple_final(s).has(method) {
            let what = if method == Derivations::LIST { "an item type of a list" } else { "a member type of a union" };
            return Err(invalid("[Schema1 §3.14.1]", &format!("a simple type final for it is not {what}")));
        }
        Ok(())
    }

    /// What `<list>` `at` of document `doc` derives [Schema2 §4.1.2.2]:
    /// the list of its item type, named or its `<simpleType>`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has both or neither of an itemType and a
    /// `<simpleType>` [Schema1 §3.14.3 src-simple-type.3], the item type is
    /// final for list or is the simple ur-type [Schema1 §3.14.6
    /// cos-st-restricts.2.1]; as `SimpleType::list`.
    fn simple_list(&mut self, at: &At<'_>, doc: usize) -> Result<Derived> {
        let inner = at.children()?.into_iter().find(|(_, l)| *l == "simpleType").map(|(c, _)| c.node);
        let item = match (self.qname(doc, at, "itemType")?, inner) {
            (Some(name), None) => self.simple_named(&name)?,
            (None, Some(node)) => Simple::Defined(self.simple_at((doc, node))?),
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid("[Schema1 §3.14.3 src-simple-type.3]", "a <list> has an itemType or a <simpleType>, not both"));
            }
        };
        self.usable(item, Derivations::LIST)?;
        let ty = self.datatype(item, "[Schema1 §3.14.6 cos-st-restricts.2.1]", "the item type of a list is atomic or a union")?;
        Ok((Simple::AnySimple, Vec::new(), list_of(ty)?))
    }

    /// What `<union>` `at` of document `doc` derives [Schema2 §4.1.2.3]:
    /// the union of its memberTypes, then its `<simpleType>`s, each union
    /// among them replaced by its members.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if a member is final for union or is the simple
    /// ur-type [Schema1 §3.14.6 cos-st-restricts.3.1]; as
    /// `SimpleType::union`, which refuses a union of no member.
    fn simple_union(&mut self, at: &At<'_>, doc: usize) -> Result<Derived> {
        let mut explicit = Vec::new();
        for word in at.token("memberTypes").unwrap_or_default().split(' ').filter(|w| !w.is_empty()) {
            let name = self.qname_text(doc, at, word)?;
            explicit.push(self.simple_named(&name)?);
        }
        for (child, _) in at.children()?.into_iter().filter(|(_, l)| *l == "simpleType") {
            explicit.push(Simple::Defined(self.simple_at((doc, child.node))?));
        }
        let (mut members, mut types) = (Vec::new(), Vec::new());
        for s in explicit {
            self.usable(s, Derivations::UNION)?;
            let inner = self.members_of(s);
            if inner.is_empty() {
                members.push(s);
            } else {
                members.extend(inner);
            }
            types.push(self.datatype(s, "[Schema1 §3.14.6 cos-st-restricts.3.1]", "the member types of a union are atomic or lists")?);
        }
        Ok((Simple::AnySimple, members, union_of(types)?))
    }

    /// [Schema1 §3.4.2 {content type} clause 1] The content type of a
    /// `<restriction>` of simple content, of children `children` in
    /// document `doc`, whose base's content type is `content`: the simple
    /// type of its `<simpleType>` \[1.1\], else `content` \[1.2\], restricted
    /// by its facets; that type itself if it gives none.
    ///
    /// # Errors
    ///
    /// As `simple_at` and `restricted`.
    pub fn content_restriction(&mut self, content: Simple, children: &[(At<'_>, &str)], doc: usize) -> Result<Simple> {
        let start = match children.iter().find(|(_, l)| *l == "simpleType") {
            Some((inner, _)) => Simple::Defined(self.simple_at((doc, inner.node))?),
            None => content,
        };
        let facets = facet_specs(children)?;
        if facets.is_empty() {
            return Ok(start);
        }
        let def = self.restricted(start, &facets)?;
        let (namespace, members) = (self.doc(doc)?.tns.clone(), self.members_of(start));
        let id = SimpleId(self.schema.simple_types.len());
        let final_ = Derivations::NONE;
        self.schema.simple_types.push(SimpleTypeDef { name: None, namespace, base: start, final_, members, def, annotation: None });
        self.simple_state.push(State::Done);
        Ok(Simple::Defined(id))
    }
}

/// `def` named `name`.
fn renamed(def: Arc<SimpleType>, name: &str) -> Arc<SimpleType> {
    Arc::new(Arc::unwrap_or_clone(def).named(name))
}

/// The list of `item` (`SimpleType::list`).
///
/// # Errors
///
/// As `SimpleType::list`.
fn list_of(item: Arc<SimpleType>) -> Result<Arc<SimpleType>> {
    SimpleType::list(item).map(Arc::new)
}

/// The union of `members` (`SimpleType::union`).
///
/// # Errors
///
/// As `SimpleType::union`.
fn union_of(members: Vec<Arc<SimpleType>>) -> Result<Arc<SimpleType>> {
    SimpleType::union(members).map(Arc::new)
}

/// A simple type definition's place held while it is read.
fn placeholder() -> SimpleTypeDef {
    SimpleTypeDef {
        name: None,
        namespace: None,
        base: Simple::AnySimple,
        final_: Derivations::NONE,
        members: Vec::new(),
        def: Arc::new(SimpleType::built_in(AtomicType::String)),
        annotation: None,
    }
}

/// The facets the facet elements among `children` specify [Schema2
/// §4.3.x.2], in order.
///
/// # Errors
///
/// SQLSTATE 22000 if one has no value, or one not of its type [Schema2
/// Appendix A]; SQLCODE -900 of a count past what 64 bits hold.
fn facet_specs(children: &[(At<'_>, &str)]) -> Result<Vec<FacetSpec>> {
    let mut out = Vec::new();
    for (at, local) in children.iter().filter(|(_, l)| FACETS.contains(l)) {
        let value = at.attr("value").ok_or_else(|| invalid("[Schema2 Appendix A]", &format!("<{local}> has a value")))?;
        let facet = match *local {
            "length" => Facet::Length(count(at, value)?),
            "minLength" => Facet::MinLength(count(at, value)?),
            "maxLength" => Facet::MaxLength(count(at, value)?),
            "totalDigits" => Facet::TotalDigits(count(at, value)?),
            "fractionDigits" => Facet::FractionDigits(count(at, value)?),
            "whiteSpace" => Facet::WhiteSpace(white_space(at, value)?),
            "pattern" => Facet::Pattern(value.into()),
            "enumeration" => Facet::Enumeration(value.into()),
            "maxInclusive" => Facet::MaxInclusive(value.into()),
            "maxExclusive" => Facet::MaxExclusive(value.into()),
            "minExclusive" => Facet::MinExclusive(value.into()),
            "minInclusive" => Facet::MinInclusive(value.into()),
            other => return Err(invalid("[Schema2 §4.1.2.1]", &format!("<{other}> is no facet"))),
        };
        out.push(FacetSpec { facet, fixed: at.boolean("fixed", false)? });
    }
    Ok(out)
}

/// The value `value` of facet element `at`, an `xs:nonNegativeInteger`
/// [Schema2 Appendix A].
///
/// # Errors
///
/// SQLSTATE 22000 if it is none; SQLCODE -900 past what 64 bits hold.
fn count(at: &At<'_>, value: &str) -> Result<u64> {
    let read = from_lexical(value, AtomicType::NonNegativeInteger)?.ok_or_else(|| at.bad("value", value))?;
    read.lexical.parse::<u64>().map_err(|_| beyond(&format!("the value {value} of <{}> is past what 64 bits hold", at.local())))
}

/// The value `value` of `<whiteSpace>` `at` [Schema2 §4.3.6.2]: preserve,
/// replace or collapse.
///
/// # Errors
///
/// SQLSTATE 22000 if it is none of them.
fn white_space(at: &At<'_>, value: &str) -> Result<WhiteSpace> {
    match value.trim() {
        "preserve" => Ok(WhiteSpace::Preserve),
        "replace" => Ok(WhiteSpace::Replace),
        "collapse" => Ok(WhiteSpace::Collapse),
        other => Err(at.bad("value", other)),
    }
}
