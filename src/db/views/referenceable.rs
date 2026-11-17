//! Referenceable views (Feature S043) and subviews (Feature S081) [ISO/IEC
//! 9075-2:2023 §11.32 SR22 to SR24, AR2, AR3, GR1e ii]: a `<referenceable
//! view specification>` checked against the catalog, and the columns it
//! makes, its self-referencing column first, REF(ST) SCOPE(V), then a
//! column of each attribute of its structured type, its `<view column
//! option>`s applied. The rules of its query, SR23m to SR23t, are
//! `referenceable/query.rs`; DROP VIEW of a superview is
//! `referenceable/drop.rs`; how its rows are read is
//! `binder/referenceable.rs`.
//!
//! \[DESIGN\] A subview is in its superview's schema \[SR23i iii\], whose
//! owner alone defines views there and holds every privilege on them. So
//! the UNDER privilege on the superview \[AR3, GR8c\] is that owner's by
//! construction and is not recorded, as of a subtable. A referenceable
//! view is read-only here, its rows changed through its basis table alone,
//! so it is not WITH CHECK OPTION either (RULES.md D81).

mod drop;
mod query;

use crate::ast::{RefGeneration, TableElement, TypedTable, ViewDefinition};
use crate::binder::{Bound, qualify};
use crate::catalog::{
    ColumnDesc, QualifiedName, RefRepresentation, StructuredDesc, TypedDesc, Underlying, ViewDesc, owner_of, proper_subtables,
    proper_supertables, sole,
};
use crate::db::{State, api, rule};
use crate::error::{At, Code, Result, SqlError};
use crate::value::DataType;
use crate::value::types::UdtName;

/// What the definition of a referenceable view makes of it.
pub(super) struct Referenceable {
    /// Its columns \[GR1e ii\].
    pub(super) columns: Vec<ColumnDesc>,
    /// Its structured type, self-referencing column and superview.
    pub(super) typed: TypedDesc,
    /// Of a derived self-referencing column, the columns of its query its
    /// values derive from \[GR1b i\].
    pub(super) derivational: Option<Vec<usize>>,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.32 SR23, SR24b, AR2] The referenceable view
    /// `name` defined by `v`, its names of schema `names`, of the owner
    /// `owner`, its query `bound`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks; -104 if `owner` holds no
    /// USAGE on its type; -900 of WITH CHECK OPTION.
    pub(super) fn referenceable_view(
        &self,
        (name, names, owner): (&QualifiedName, &str, &str),
        v: &ViewDefinition,
        bound: &Bound,
    ) -> Result<Referenceable> {
        let Some(spec) = v.typed.as_deref() else { return Err(api(format!("[DESIGN] view {name} is not referenceable"))) };
        let (clause, elements) = (&spec.0, spec.1.as_slice());
        if v.check_option.is_some() {
            let msg = format!("[DESIGN] referenceable view {name} is read-only here, so it is not WITH CHECK OPTION (RULES.md D81)");
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        let st = self.view_type(clause, (names, owner))?;
        let (typed, sup) = match &clause.under {
            Some(under) => {
                let sup = self.superview_of((name, &st), &qualify(under, names)).or_at(under.at)?;
                let t = sup.typed.clone().ok_or_else(|| api(format!("[DESIGN] {} is not referenceable", sup.name)))?;
                (TypedDesc { udt: st.name.clone(), supertable: Some(sup.name.clone()), ..t }, Some(sup))
            }
            None => (self.root_typed(&st, elements).or_at(clause.udt.at)?, None),
        };
        if st.attribute(&typed.self_referencing).is_some() {
            let item = if sup.is_some() { "SR24b i" } else { "SR24b ii" };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 {item}] self-referencing column {} of {name} is named as an attribute of {}",
                typed.self_referencing, st.name
            )));
        }
        let m = usize::from(matches!(typed.generation, RefGeneration::User));
        attribute_types(&st, bound, m)?;
        self.basis_rules(&v.query, names, (&st, &typed), (sup, bound))?;
        let columns = self.view_columns((name, names), (&st, &typed), elements, bound)?;
        let derivational = derivational(self, &st, &typed);
        Ok(Referenceable { columns, typed, derivational })
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23c, AR2] The structured type `clause`
    /// names, its names of schema `names`, on which `owner` holds USAGE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names no structured type; -104 if `owner` holds
    /// no USAGE on it.
    fn view_type(&self, clause: &TypedTable, (names, owner): (&str, &str)) -> Result<StructuredDesc> {
        let qn = qualify(&clause.udt, names);
        let Some(st) = self.catalog.structured_type(&qn) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.32 SR23c] {qn} is no structured type, as a referenceable view's type is"))
                .or_at(clause.udt.at));
        };
        if !st.usage_of(sole(&*self.catalog, owner), owner_of(&*self.catalog, &qn.auth)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.32 AR2] {owner} holds no USAGE on type {qn}");
            return Err(SqlError::new(Code::Privilege, msg).or_at(clause.udt.at));
        }
        Ok(st.clone())
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23h iv] What makes a referenceable
    /// view of type `st` that is no subview typed, of the `elements` its
    /// definition writes: its self-referencing column, USER GENERATED of a
    /// user-defined reference representation, DERIVED of a derived one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its generation is not its type's representation.
    fn root_typed(&self, st: &StructuredDesc, elements: &[TableElement]) -> Result<TypedDesc> {
        let found = elements.iter().find_map(|e| match e {
            TableElement::SelfReferencing(name, generation, at) => Some((name.clone(), *generation, *at)),
            TableElement::Column(_)
            | TableElement::Constraint(_)
            | TableElement::Like(_)
            | TableElement::Options(_)
            | TableElement::Period(_) => None,
        });
        let Some((name, generation, at)) = found else {
            return Err(api(format!("[DESIGN] a referenceable view of {} has no <self-referencing column specification>", st.name)));
        };
        // [§11.51 SR8b] A subtype's reference representation is its
        // maximal supertype's.
        let maximal = self.catalog.structured_type(&st.maximal()).unwrap_or(st);
        let (clause, needed) = match (generation, &maximal.reference) {
            (RefGeneration::User, RefRepresentation::User(_)) | (RefGeneration::Derived, RefRepresentation::Derived(_)) => {
                return Ok(TypedDesc { udt: st.name.clone(), self_referencing: name, generation, supertable: None });
            }
            (RefGeneration::User, _) => ("SR23h iv 1 A", "a user-defined"),
            (RefGeneration::Derived, _) => ("SR23h iv 2 A", "a derived"),
            (RefGeneration::System, _) => ("SR23h iii", "no system-defined"),
        };
        Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.32 {clause}] self-referencing column {name} is {}, so REF({}) has {needed} representation, not {}",
            generation.name(),
            st.name,
            maximal.reference.name()
        ))
        .or_at(at))
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23i i to iii, SR23d] The direct
    /// superview `sq` of the subview `name` of type `st`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no referenceable view, `st` is no direct
    /// subtype of its type, it is in another schema, or its subtable
    /// family has a view of type `st`.
    fn superview_of(&self, (name, st): (&QualifiedName, &StructuredDesc), sq: &QualifiedName) -> Result<ViewDesc> {
        let Some((sup, typed)) = self.catalog.view(sq).and_then(|v| v.typed.as_ref().map(|t| (v, t))) else {
            let what = match (self.catalog.table(sq), self.catalog.view(sq)) {
                (Some(_), _) => "a base table",
                (None, Some(_)) => "a view that is not referenceable",
                (None, None) => "not defined",
            };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23i i] {sq} is {what}, and a direct superview is a referenceable view"
            )));
        };
        if st.supertype.as_ref() != Some(&typed.udt) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23i ii] type {} is no direct subtype of {}, the type of {sq}",
                st.name, typed.udt
            )));
        }
        if sq.auth != name.auth {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23i iii] {sq} is not in schema {}, as its subview {name} is",
                name.auth
            )));
        }
        let root = proper_supertables(&*self.catalog, sq).pop().unwrap_or_else(|| sq.clone());
        let family = std::iter::once(root.clone()).chain(proper_subtables(&*self.catalog, &root));
        let of_type = |v: &QualifiedName| self.catalog.view(v).and_then(|d| d.typed.as_ref()).is_some_and(|t| t.udt == st.name);
        if let Some(other) = family.into_iter().find(|v| of_type(v)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23d] {other} of the subtable family of {sq} is of type {} already",
                st.name
            )));
        }
        Ok(sup.clone())
    }

    /// [ISO/IEC 9075-2:2023 §11.32 GR1e ii, SR23u] The columns of the
    /// referenceable view `name` of type `st`, made typed by `typed`, its
    /// names of schema `names`, of the `<view column option>`s among
    /// `elements`, its query `bound`: its self-referencing column, then a
    /// column of each attribute, its scope that of its option, if it has
    /// one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an option breaks a rule of SR23u.
    fn view_columns(
        &self,
        (name, names): (&QualifiedName, &str),
        (st, typed): (&StructuredDesc, &TypedDesc),
        elements: &[TableElement],
        bound: &Bound,
    ) -> Result<Vec<ColumnDesc>> {
        let column = |c: &str, data_type: DataType, not_null: bool| ColumnDesc {
            name: c.to_owned(),
            data_type,
            not_null,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        };
        let rst = DataType::reference(st.structured_type(), Some(UdtName::new(&name.auth, &name.ident)));
        let mut columns = vec![column(&typed.self_referencing, rst, true)];
        let m = usize::from(matches!(typed.generation, RefGeneration::User));
        for (a, c) in st.attributes.iter().zip(bound.columns.iter().skip(m)) {
            columns.push(ColumnDesc { collation: c.collation.clone(), ..column(&a.name, a.data_type.clone(), c.not_null) });
        }
        let mut optioned: Vec<&str> = Vec::new();
        for o in elements.iter().filter_map(|e| if let TableElement::Options(o) = e { Some(o) } else { None }) {
            let (attribute, at) = (o.column.0.as_str(), o.column.1);
            if optioned.contains(&attribute) {
                return Err(
                    rule(format!("[ISO/IEC 9075-2:2023 §11.32 SR23u ii] attribute {attribute} has two <view column option>s")).or_at(at)
                );
            }
            optioned.push(attribute);
            let scoped = self.scoped_option((st, attribute), (o.scope.as_ref(), names)).or_at(at)?;
            if let Some(c) = columns.iter_mut().find(|c| c.name == attribute) {
                c.data_type = scoped;
            }
        }
        Ok(columns)
    }

    /// [ISO/IEC 9075-2:2023 §11.32 SR23u i, iii, GR1e ii 2 C] The declared
    /// type of the column of `attribute` of `st` given the scope `scope`,
    /// its names of schema `names`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `attribute` is no originally-defined attribute of
    /// `st`, or of no reference type, or `scope` is no referenceable table
    /// of its referenced type.
    fn scoped_option(
        &self,
        (st, attribute): (&StructuredDesc, &str),
        (scope, names): (Option<&crate::ast::TableName>, &str),
    ) -> Result<DataType> {
        let Some((_, a)) = st.attribute(attribute).filter(|(_, a)| a.defined_by == st.name) else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23u i] {attribute} is no originally-defined attribute of {}",
                st.name
            )));
        };
        let DataType::Reference(rt) = &a.data_type else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23u iii 1] attribute {attribute} is of no reference type, so it takes no scope"
            )));
        };
        let Some(scope) = scope else { return Err(api("[DESIGN] a <view column option> has no <scope clause>")) };
        let sq = qualify(scope, names);
        let typed =
            self.catalog.table(&sq).and_then(|t| t.typed.as_ref()).or_else(|| self.catalog.view(&sq).and_then(|v| v.typed.as_ref()));
        let Some(typed) = typed else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.32 SR23u iii 2] the scope {sq} of {attribute} is no referenceable table")));
        };
        if typed.udt.auth != rt.referenced.name.schema || typed.udt.ident != rt.referenced.name.name {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.32 SR23u iii 3] the scope {sq} of {attribute} is of type {}, not {}",
                typed.udt, rt.referenced.name
            )));
        }
        Ok(DataType::reference((*rt.referenced).clone(), Some(UdtName::new(&sq.auth, &sq.ident))))
    }
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23j, SR23l] That the query `bound` of a
/// referenceable view of type `st` has `m` columns before one of each
/// attribute, each of the attribute's declared type [§9.30].
///
/// # Errors
///
/// SQLCODE -103 if it has not.
fn attribute_types(st: &StructuredDesc, bound: &Bound, m: usize) -> Result<()> {
    let n = st.attributes.len();
    if bound.columns.len() != n.saturating_add(m) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.32 SR23j] the query of a referenceable view of {} has {} columns, not {}",
            st.name,
            bound.columns.len(),
            n.saturating_add(m)
        )));
    }
    if let Some((a, c)) = st.attributes.iter().zip(bound.columns.iter().skip(m)).find(|(a, c)| !identical(&c.data_type, &a.data_type)) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.32 SR23l, §9.30] the column of attribute {} is of {}, not {}, its declared type",
            a.name, c.data_type, a.data_type
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.30] Whether `a` and `b` are compatible and of
/// the same characteristics: of reference types, of one referenced type,
/// whatever their scopes.
fn identical(a: &DataType, b: &DataType) -> bool {
    if let (DataType::Reference(x), DataType::Reference(y)) = (a, b) {
        return x.referenced.name == y.referenced.name;
    }
    a == b
}

/// [ISO/IEC 9075-2:2023 §11.32 GR1b i] Of a view of type `st` made typed
/// by `typed` whose self-referencing column is derived, the positions in
/// its query of the attributes its references derive from, its maximal
/// supertype's derivational attributes.
fn derivational(db: &State, st: &StructuredDesc, typed: &TypedDesc) -> Option<Vec<usize>> {
    if !matches!(typed.generation, RefGeneration::Derived) {
        return None;
    }
    let maximal = db.catalog.structured_type(&st.maximal()).unwrap_or(st);
    let RefRepresentation::Derived(names) = &maximal.reference else { return None };
    names.iter().map(|n| st.attribute(n).map(|(i, _)| i)).collect()
}

/// [ISO/IEC 9075:1992 §11.19 GR6] What the columns of a view are drawn
/// from, `u` those of its query: of a derived self-referencing column,
/// which is first, those of the query's columns at `derivational`.
pub(super) fn prefixed(u: Underlying, derivational: Option<&[usize]>) -> Underlying {
    let Some(at) = derivational else { return u };
    let first = at.iter().filter_map(|&k| u.columns.get(k)).flatten().cloned().collect();
    let columns = std::iter::once(first).chain(u.columns).collect();
    Underlying { leaves: u.leaves, columns }
}
