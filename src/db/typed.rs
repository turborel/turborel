//! Typed tables (Feature S051) and subtables (Feature S081) [ISO/IEC
//! 9075-2:2023 §4.17.9, §11.3 SR11, AR3, AR4, GR4f, GR4h, GR4i]: a `<typed
//! table clause>` effectively replaced by the `<table element list>` it
//! stands for \[SR11i\], its self-referencing column first, then a column of
//! each attribute of its structured type, its `<column options>` applied,
//! then its table constraints. The values an INSERT gives a typed table's
//! self-referencing column, and what an INSERT or UPDATE of one may not do,
//! are `typed/values.rs`; DROP TABLE of a supertable is `typed/drop.rs`.
//!
//! \[DESIGN\] A subtable is in its supertable's schema \[SR11d iii\], whose
//! owner alone defines tables there and holds every privilege on them: the
//! UNDER privilege on the supertable \[AR3\] and those on the inherited
//! columns \[GR6\] are that owner's by construction, so neither is
//! recorded. A table hierarchy is no deeper than its type hierarchy, each
//! subtable's type a direct subtype of its supertable's \[SR11d ii\], so
//! the bound on a type hierarchy, `MAX_HIERARCHY_DEPTH` (RULES.md D81),
//! bounds it too.

mod drop;
mod options;
pub(in crate::db) mod values;

pub(in crate::db) use values::{derived_reference, insertable_into, system_reference};

use super::contents::{Definer, Element, Kept};
use super::sequences::{DEFAULT_TYPE, generator};
use super::{State, api, rule};
use crate::ast::{
    ColumnConstraint, ColumnConstraintKind, ColumnDefinition, ConstraintAttributes, DefaultOption, RefGeneration, SequenceOptions,
    TableConstraintKind, TableDefinition, TableElement, TableName, TypeName, TypedTable,
};
use crate::binder::qualify;
use crate::catalog::{
    AttributeDesc, Generated, MAX_HIERARCHY_DEPTH, QualifiedName, RefRepresentation, SequenceDesc, StructuredDesc, TableDesc, TypedDesc,
    owner_of, sole,
};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;
use crate::value::DataType;
use crate::value::types::UdtName;

/// A `<column definition>` a typed table's definition stands for, what it
/// keeps beyond it, and whether it is an originally-defined column
/// \[SR11e\], one `<column options>` may name \[SR11g i\].
pub(super) struct TypedColumn {
    /// The `<column definition>`.
    def: ColumnDefinition,
    /// Its supercolumn's default, or its generation.
    kept: Kept,
    /// Of an originally-defined attribute.
    original: bool,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.3 SR11i] The `<table element>`s of the
    /// typed table `t`, of `<typed table clause>` `typed`, defined by
    /// `definer`, the tables `earlier` in the same `<schema definition>`
    /// among those its supertable may be: CD0, CD1 to CDn, then its table
    /// constraints.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule of SR11 it breaks; -104 if the owner holds
    /// no USAGE on its type \[AR4\].
    pub(super) fn typed_elements<'t>(
        &self,
        definer: Definer<'_>,
        (t, typed): (&'t TableDefinition, &TypedTable),
        earlier: &[TableDesc],
    ) -> Result<Vec<Element<'t>>> {
        let (table, owner, names, _) = definer;
        let st = self.typed_type(typed, (names, owner))?;
        let mut columns = match &typed.under {
            Some(under) => {
                let sup = self.supertable_of((table, &st), under, (names, earlier))?;
                subtable_columns((table, &st), &sup, typed.udt.at)?
            }
            None => self.root_columns((table, &st), t, typed.udt.at)?,
        };
        self.optioned((table, names), &mut columns, t)?;
        let mut elements: Vec<Element<'t>> =
            columns.into_iter().map(|c| Element::Copied(Box::new((TableElement::Column(c.def), c.kept)))).collect();
        elements.extend(t.elements.iter().filter(|e| matches!(e, TableElement::Constraint(_))).map(Element::Written));
        Ok(elements)
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR11a, AR4] The structured type `typed`
    /// names, its names of schema `names`, on which `owner` holds USAGE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names no structured type; -104 if `owner` holds
    /// no USAGE on it.
    fn typed_type(&self, typed: &TypedTable, (names, owner): (&str, &str)) -> Result<StructuredDesc> {
        let qn = qualify(&typed.udt, names);
        let Some(st) = self.catalog.structured_type(&qn) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR11a] {qn} is no structured type, as a typed table's type is"))
                .or_at(typed.udt.at));
        };
        if !st.usage_of(sole(&*self.catalog, owner), owner_of(&*self.catalog, &qn.auth)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.3 AR4] {owner} holds no USAGE on type {qn}");
            return Err(SqlError::new(Code::Privilege, msg).or_at(typed.udt.at));
        }
        Ok(st.clone())
    }

    /// The table `qn`, of the catalog or `earlier` in the same `<schema
    /// definition>`.
    fn table_in(&self, qn: &QualifiedName, earlier: &[TableDesc]) -> Option<TableDesc> {
        self.catalog.table(qn).cloned().or_else(|| earlier.iter().find(|d| d.name == *qn).cloned())
    }

    /// [ISO/IEC 9075-2:2023 §4.17.9] The maximal supertable of the table
    /// `qn`: itself, or the last of its proper supertables.
    fn maximal_table(&self, qn: &QualifiedName, earlier: &[TableDesc]) -> QualifiedName {
        let mut at = qn.clone();
        for _ in 0..MAX_HIERARCHY_DEPTH {
            let Some(up) = self.table_in(&at, earlier).and_then(|d| d.typed).and_then(|t| t.supertable) else { break };
            at = up;
        }
        at
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR11d i to iv] The direct supertable
    /// `under` names, its names of schema `names`, of the subtable `table`
    /// of type `st`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no typed base table, `st` is no direct
    /// subtype of its type, it is in another schema, or its subtable
    /// family has a table of type `st`.
    fn supertable_of(
        &self,
        (table, st): (&QualifiedName, &StructuredDesc),
        under: &TableName,
        (names, earlier): (&str, &[TableDesc]),
    ) -> Result<TableDesc> {
        let sq = qualify(under, names);
        let fail = |msg: String| Err(rule(msg).or_at(under.at));
        let Some(sup) = self.table_in(&sq, earlier) else {
            let what = if self.catalog.view(&sq).is_some() { "a view" } else { "not defined" };
            return fail(format!("[ISO/IEC 9075-2:2023 §11.3 SR11d i] {sq} is {what}, and a direct supertable is a base table"));
        };
        let Some(typed) = &sup.typed else {
            return fail(format!("[ISO/IEC 9075-2:2023 §11.3 SR11d ii] {sq} is no typed table, so it is no direct supertable"));
        };
        if st.supertype.as_ref() != Some(&typed.udt) {
            return fail(format!(
                "[ISO/IEC 9075-2:2023 §11.3 SR11d ii] type {} is no direct subtype of {}, the type of {sq}",
                st.name, typed.udt
            ));
        }
        if sq.auth != table.auth {
            return fail(format!("[ISO/IEC 9075-2:2023 §11.3 SR11d iii] {sq} is not in schema {}, as its subtable {table} is", table.auth));
        }
        let root = self.maximal_table(&sq, earlier);
        let members = self.catalog.tables().into_iter().cloned().chain(earlier.iter().cloned());
        let same =
            members.filter(|d| d.typed.as_ref().is_some_and(|t| t.udt == st.name)).find(|d| self.maximal_table(&d.name, earlier) == root);
        if let Some(other) = same {
            return fail(format!(
                "[ISO/IEC 9075-2:2023 §11.3 SR11d iv] {} of the subtable family of {sq} is of type {} already",
                other.name, st.name
            ));
        }
        Ok(sup)
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR11c, SR11f] The columns of the typed
    /// table `table` of type `st` that is no subtable, defined by `t`,
    /// whose type is named at `at`: CD0, then one of each attribute.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its self-referencing column's generation is not its
    /// reference type's, or a derived one lacks its constraints.
    fn root_columns(&self, (table, st): (&QualifiedName, &StructuredDesc), t: &TableDefinition, at: usize) -> Result<Vec<TypedColumn>> {
        let Some((name, generation, written)) = self_specification(t) else {
            return Err(api(format!("[DESIGN] typed table {table} has no <self-referencing column specification>")));
        };
        // [§11.51 SR8b] A subtype's reference representation is its
        // maximal supertype's.
        let maximal = self.catalog.structured_type(&st.maximal()).unwrap_or(st);
        let generated = match (generation, &maximal.reference) {
            (RefGeneration::System, RefRepresentation::System) => Some(Generated::SystemReference(family_generator(table, written)?)),
            (RefGeneration::User, RefRepresentation::User(_)) => None,
            (RefGeneration::Derived, RefRepresentation::Derived(derivational)) => {
                derived_constraints(t, derivational)?;
                Some(Generated::DerivedReference(derivational.clone()))
            }
            (RefGeneration::System, _) | (RefGeneration::User, _) | (RefGeneration::Derived, _) => {
                return Err(generation_refused((name, generation), st, &maximal.reference).or_at(written));
            }
        };
        let mut columns =
            vec![TypedColumn { def: self_column(name, written, (st, table)), kept: Kept::new(None, generated), original: false }];
        for a in &st.attributes {
            let default = a.default.as_deref().map(default_option).transpose()?;
            columns.push(TypedColumn { def: attribute_column(a, at, default), kept: Kept::default(), original: a.defined_by == st.name });
        }
        Ok(columns)
    }

    /// [ISO/IEC 9075-2:2023 §11.3 GR4f, GR4h, GR4i] What makes the table
    /// `t` typed, if it is, its names of schema `names`, the tables
    /// `earlier` in the same `<schema definition>` among its supertables:
    /// its type, its self-referencing column, the first, and its direct
    /// supertable; a subtable's column named and generated as its
    /// supertable's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `typed_elements` did not find what it finds.
    pub(super) fn typed_desc(&self, names: &str, t: &TableDefinition, earlier: &[TableDesc]) -> Result<Option<TypedDesc>> {
        let Some(typed) = &t.typed else { return Ok(None) };
        let udt = qualify(&typed.udt, names);
        if let Some(under) = &typed.under {
            let sq = qualify(under, names);
            let of_super = self.table_in(&sq, earlier).and_then(|d| d.typed);
            let s = of_super.ok_or_else(|| api(format!("[DESIGN] {sq} is no typed table")))?;
            return Ok(Some(TypedDesc { udt, self_referencing: s.self_referencing, generation: s.generation, supertable: Some(sq) }));
        }
        let (name, generation, _) = self_specification(t).ok_or_else(|| api(format!("[DESIGN] {udt} has no self-referencing column")))?;
        Ok(Some(TypedDesc { udt, self_referencing: name.to_owned(), generation, supertable: None }))
    }

    /// [ISO/IEC 9075-2:2023 §4.17.9] Each proper subtable of `table`, its
    /// constraints changed, of its columns' nullability again: of its own
    /// constraints and those it inherits, the nearest supertable first.
    pub(super) fn renullified(&mut self, table: &QualifiedName) {
        let subtables = crate::catalog::proper_subtables(&*self.catalog, table);
        if subtables.is_empty() {
            return;
        }
        for sub in subtables {
            let Some(mut desc) = self.catalog.table(&sub).cloned() else { continue };
            desc.refresh_not_null();
            self.inherited_nullability(&mut desc, &[]);
            let mut catalog = self.catalog.duplicate();
            catalog.replace_table(desc);
            self.catalog = std::sync::Arc::from(catalog);
        }
    }

    /// [ISO/IEC 9075-2:2023 §4.17.9, §11.3 SR11f iii] Of a subtable
    /// `desc`, each inherited column known not nullable as its direct
    /// supercolumn is, of the supertable in the catalog or `earlier` in
    /// the same `<schema definition>`: no constraint of the subtable's
    /// own, as every constraint of a supertable is checked of its
    /// subtables' rows.
    pub(super) fn inherited_nullability(&self, desc: &mut TableDesc, earlier: &[TableDesc]) {
        let Some(sq) = desc.typed.as_ref().and_then(|t| t.supertable.clone()) else { return };
        let Some(sup) = self.table_in(&sq, earlier) else { return };
        for c in &mut desc.columns {
            if sup.columns.iter().any(|s| s.name == c.name && s.not_null) {
                c.not_null = true;
            }
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11d vi, vii, SR11f] The columns of the
/// subtable `table` of type `st`, of the direct supertable `sup`, whose
/// type is named at `at`: CD0, named and generated as the supertable's
/// self-referencing column, then one of each attribute, an inherited
/// one's default its direct supercolumn's.
///
/// # Errors
///
/// SQLCODE -910 if `sup` lacks its self-referencing column, or an
/// attribute's default no longer reads.
fn subtable_columns((table, st): (&QualifiedName, &StructuredDesc), sup: &TableDesc, at: usize) -> Result<Vec<TypedColumn>> {
    let name = sup.typed.as_ref().map(|t| t.self_referencing.as_str()).unwrap_or_default();
    let reference = sup.columns.iter().find(|c| c.name == name);
    let generated = reference.ok_or_else(|| api(format!("[DESIGN] {} has no self-referencing column {name}", sup.name)))?.generated.clone();
    let mut columns = vec![TypedColumn { def: self_column(name, at, (st, table)), kept: Kept::new(None, generated), original: false }];
    for a in &st.attributes {
        let original = a.defined_by == st.name;
        let (default, kept) = if original {
            (a.default.as_deref().map(default_option).transpose()?, Kept::default())
        } else {
            let supercolumn = sup.columns.iter().find(|c| c.name == a.name).and_then(|c| c.default.clone());
            (None, Kept::new(supercolumn, None))
        };
        columns.push(TypedColumn { def: attribute_column(a, at, default), kept, original });
    }
    Ok(columns)
}

/// The `<self-referencing column specification>` of `t`, if it has one:
/// its column's name, its generation, and where it is written.
fn self_specification(t: &TableDefinition) -> Option<(&str, RefGeneration, usize)> {
    t.elements.iter().find_map(|e| match e {
        TableElement::SelfReferencing(name, generation, at) => Some((name.as_str(), *generation, *at)),
        TableElement::Column(_) | TableElement::Constraint(_) | TableElement::Like(_) | TableElement::Options(_) => None,
    })
}

/// [ISO/IEC 9075-2:2023 §11.3 GR4f iii] The sequence generator a subtable
/// family whose maximal supertable is `table`, defined at `at`, draws its
/// REF values from: ascending from 1 by 1, of NO CYCLE, so no value is
/// given twice; its epoch still to give (`give_epochs`).
///
/// # Errors
///
/// As `generator`.
fn family_generator(table: &QualifiedName, at: usize) -> Result<SequenceDesc> {
    let made = generator(table.clone(), DEFAULT_TYPE, &SequenceOptions::default(), at)?;
    Ok(SequenceDesc { start: 1, minimum: 1, origin: 1, ..made })
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11c iii to v] The refusal of the
/// self-referencing column `name` of generation `generation`, of a table
/// of type `st`, whose reference type has representation `rep`.
fn generation_refused((name, generation): (&str, RefGeneration), st: &StructuredDesc, rep: &RefRepresentation) -> SqlError {
    let (clause, needed) = match generation {
        RefGeneration::System => ("SR11c iii", "a system-defined"),
        RefGeneration::User => ("SR11c iv", "a user-defined"),
        RefGeneration::Derived => ("SR11c v", "a derived"),
    };
    rule(format!(
        "[ISO/IEC 9075-2:2023 §11.3 {clause}] self-referencing column {name} is {}, so REF({}) has {needed} representation, not {}",
        generation.name(),
        st.name,
        rep.name()
    ))
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11c vi] That `t`, of a type whose
/// references derive from the attributes `derivational`, has a UNIQUE or
/// PRIMARY KEY constraint on them in order, and, of UNIQUE, `<column
/// options>` of each that are NOT NULL.
///
/// # Errors
///
/// SQLCODE -103 if it lacks one.
fn derived_constraints(t: &TableDefinition, derivational: &[String]) -> Result<()> {
    let unique = t.elements.iter().find_map(|e| match e {
        TableElement::Constraint(c) => match &c.kind {
            TableConstraintKind::Unique { primary, columns, .. } => (columns.as_slice() == derivational).then_some(*primary),
            TableConstraintKind::ForeignKey { .. } | TableConstraintKind::Check(_) => None,
        },
        TableElement::Column(_) | TableElement::Like(_) | TableElement::SelfReferencing(..) | TableElement::Options(_) => None,
    });
    let list = derivational.join(", ");
    let Some(primary) = unique else {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.3 SR11c vi 1] its references derive from ({list}), so the table has a UNIQUE or PRIMARY KEY constraint on them in that order"
        )));
    };
    let not_null = |a: &String| {
        t.elements.iter().any(|e| {
            matches!(e, TableElement::Options(o) if o.column.0 == *a && o.constraints.iter().any(|c| matches!(c.kind, ColumnConstraintKind::NotNull)))
        })
    };
    if let (false, Some(a)) = (primary, derivational.iter().find(|a| !not_null(a))) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.3 SR11c vi 2] its references derive from attribute {a}, so its <column options> are NOT NULL, as the constraint on ({list}) is no PRIMARY KEY"
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11c vii, SR11d vii] CD0, `name` REF(ST)
/// SCOPE(TN) UNIQUE NOT NULL, of the table `table` of type `st`, written
/// at `at`.
fn self_column(name: &str, at: usize, (st, table): (&StructuredDesc, &QualifiedName)) -> ColumnDefinition {
    let constraint = |kind| ColumnConstraint { name: None, at, kind, attributes: ConstraintAttributes::default() };
    let rst = DataType::reference(st.structured_type(), Some(UdtName::new(&table.auth, &table.ident)));
    ColumnDefinition {
        name: name.to_owned(),
        at,
        data_type: Some(TypeName::Data(rst)),
        default: None,
        generated: None,
        constraints: vec![constraint(ColumnConstraintKind::Unique { primary: false }), constraint(ColumnConstraintKind::NotNull)],
        collate: None,
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11f] CDi, `CNi DTi DCi`, of the attribute
/// `a`, written at `at`, its `<default option>` `default`.
fn attribute_column(a: &AttributeDesc, at: usize, default: Option<DefaultOption>) -> ColumnDefinition {
    ColumnDefinition {
        name: a.name.clone(),
        at,
        data_type: Some(TypeName::Data(a.data_type.clone())),
        default,
        generated: None,
        constraints: Vec::new(),
        collate: a.collation.as_ref().map(|q| TableName { auth: Some(q.auth.clone()), ident: q.ident.clone(), at }),
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11f iv 1] The `<default option>` an
/// attribute's descriptor keeps as written.
///
/// # Errors
///
/// SQLCODE -910 if it no longer reads as one.
fn default_option(text: &str) -> Result<DefaultOption> {
    Parser::new(text)?.default_option()
}
