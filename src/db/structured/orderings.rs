//! [ISO/IEC 9075-2:2023 §11.65, §11.66, §4.9.5] User-defined orderings:
//! CREATE ORDERING, of one form and category across a subtype family,
//! and DROP ORDERING, RESTRICT refused while a definition compares values
//! of the type.

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::{State, rule};
use super::user_defined;
use crate::ast::{DataAccess, OrderingCategory as Written, OrderingDefinition, OrderingForm};
use crate::binder::qualify;
use crate::catalog::{OrderingCategory, OrderingDesc, QualifiedName, SqlRoutineDesc, StructuredDesc, owner_of, udt_qualified};
use crate::error::{At, Result};
use crate::txn::LockMode;
use crate::value::types::MAX_TYPE_NESTING;
use crate::value::{DataType, Spelled};

/// The suffix of the specific name of a STATE ordering's function when
/// none is written, after the type's name (UV102) [§11.65 SR6a iv].
const EQUALS_SUFFIX: &str = "_EQUALS";

impl State {
    /// [ISO/IEC 9075-2:2023 §4.9.5] The ordering of the comparison type of
    /// the structured type `desc`: the first of its type precedence list
    /// with one.
    pub(in crate::db) fn comparison_of(&self, desc: &StructuredDesc) -> Option<OrderingDesc> {
        let named = std::iter::once(desc.name.clone()).chain(desc.supertypes.iter().map(udt_qualified));
        named.into_iter().find_map(|t| self.catalog.ordering(&t).cloned())
    }

    /// [ISO/IEC 9075-2:2023 §11.65] CREATE ORDERING `o`, its names in schema
    /// `schema` if it names none: the type's ordering form, category and
    /// function [GR1 to GR4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks.
    pub(in crate::db) fn create_ordering(&mut self, schema: &str, o: &OrderingDefinition) -> Result<Defined> {
        let qn = qualify(&o.udt, schema);
        let structured = self.catalog.structured_type(&qn).cloned();
        let data_type = match (&structured, self.catalog.user_type(&qn)) {
            (Some(s), _) => s.data_type(),
            (None, Some(d)) => d.data_type(),
            (None, None) => {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.65 SR1] there is no user-defined type {qn}")).or_at(o.udt.at));
            }
        };
        if self.catalog.ordering(&qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.65 SR2] type {qn} has an ordering already: drop it first")).or_at(o.udt.at));
        }
        if let Some(s) = &structured {
            self.family_ordered(s, o).or_at(o.udt.at)?;
        }
        let category = match &o.category {
            Written::State(sn) => {
                OrderingCategory::State(Some(self.state_ordering(structured.as_ref(), (&qn, o.form), sn.as_ref()).or_at(o.udt.at)?))
            }
            Written::Relative(d) | Written::Map(d) => {
                let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
                let f = self.function_designated(d, (schema, &owner), "§11.65 SR6b")?;
                ordering_function(&f, &o.category, (&data_type, o.form)).or_at(d.name.at)?;
                if let Written::Relative(_) = &o.category {
                    OrderingCategory::Relative(f.specific)
                } else {
                    OrderingCategory::Map(f.specific)
                }
            }
        };
        let locks = category_function(&category).into_iter().map(|f| (f, LockMode::Shared)).collect();
        let mut catalog = self.catalog.duplicate();
        catalog.put_ordering(OrderingDesc { udt: qn, form: o.form, category });
        self.catalog = Arc::from(catalog);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.65 SR3 to SR5, §4.9.5] That the ordering
    /// `o` of `s` agrees with its subtype family's: of its direct
    /// supertype's comparison form, and of MAP if that is MAP; RELATIVE and
    /// STATE of a maximal supertype alone; and each proper subtype's own
    /// ordering of its form.
    fn family_ordered(&self, s: &StructuredDesc, o: &OrderingDefinition) -> Result<()> {
        let map = matches!(o.category, Written::Map(_));
        let direct = s.supertype.as_ref().and_then(|t| self.catalog.structured_type(t));
        if let Some(t) = direct {
            let above = self.comparison_of(t);
            if above.as_ref().map(|a| a.form) != Some(o.form) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.65 SR3] the ordering of {} is of its direct supertype {}'s comparison form",
                    s.name, t.name
                )));
            }
            if !map {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.65 SR4] the ordering of {}, no maximal supertype, is by MAP", s.name)));
            }
            if !above.is_some_and(|a| matches!(a.category, OrderingCategory::Map(_))) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.65 SR5] type {}'s direct supertype {} compares by MAP, so it does too",
                    s.name, t.name
                )));
            }
        }
        for sub in self.proper_subtypes(&s.name) {
            let below = self.catalog.ordering(&sub.name).cloned();
            if below.is_some_and(|b| b.form != o.form || !map || !matches!(b.category, OrderingCategory::Map(_))) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.65 SR3, SR5] subtype {}'s ordering is not of this form by MAP",
                    sub.name
                )));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.65 SR6a] The specific name of the STATE
    /// ordering of the type `qn` of form `form`, `structured` if it is one:
    /// of a structured type, EQUALS ONLY, no attribute of a user-defined type
    /// of no comparison type; `sn` if written, in the type's schema, else
    /// the type's name and `EQUALS_SUFFIX` (UV102), no routine's.
    fn state_ordering(
        &self,
        structured: Option<&StructuredDesc>,
        (qn, form): (&QualifiedName, OrderingForm),
        sn: Option<&crate::ast::TableName>,
    ) -> Result<QualifiedName> {
        let Some(s) = structured else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.65 SR6a i] distinct type {qn} is ordered by no STATE")));
        };
        if form != OrderingForm::Equals {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.65 SR6a ii] an ordering by STATE is EQUALS ONLY".to_owned()));
        }
        if let Some(a) = s.attributes.iter().find(|a| self.nc_ordered(&a.data_type, 0)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.65 SR6a iii] attribute {} of {qn} is of a type of no comparison type",
                a.name
            )));
        }
        let Some(written) = sn else {
            let mut name = QualifiedName { auth: qn.auth.clone(), ident: format!("{}{EQUALS_SUFFIX}", qn.ident) };
            let mut n: u64 = 0;
            while self.specific_in_use(&name) || self.ordering_named(&name) {
                n = n.saturating_add(1);
                name.ident = format!("{}{EQUALS_SUFFIX}_{n}", qn.ident);
            }
            return Ok(name);
        };
        let name = qualify(written, &qn.auth);
        if name.auth != qn.auth || self.specific_in_use(&name) || self.ordering_named(&name) {
            return Err(
                rule(format!("[ISO/IEC 9075-2:2023 §11.65 SR6a iv] {name} is of schema {} and no routine's", qn.auth)).or_at(written.at)
            );
        }
        Ok(name)
    }

    /// Whether an ordering's function has the specific name `name`.
    fn ordering_named(&self, name: &QualifiedName) -> bool {
        self.catalog.orderings().iter().any(|o| o.function() == Some(name))
    }

    /// [ISO/IEC 9075-2:2023 §4.2.4] Whether `t` is UDT-NC-ordered: a
    /// user-defined type of no comparison type, or holds one, at most
    /// `MAX_TYPE_NESTING` deep.
    pub(in crate::db) fn nc_ordered(&self, t: &DataType, depth: usize) -> bool {
        if depth > MAX_TYPE_NESTING {
            return false;
        }
        let deeper = depth.saturating_add(1);
        match t {
            DataType::Distinct(d) => self.catalog.ordering(&QualifiedName { auth: d.schema.clone(), ident: d.name.clone() }).is_none(),
            DataType::Structured(s) => {
                self.catalog.structured_type(&udt_qualified(&s.name)).is_none_or(|desc| self.comparison_of(desc).is_none())
            }
            DataType::Array(a) => self.nc_ordered(&a.element, deeper),
            DataType::Multiset(m) => self.nc_ordered(&m.element, deeper),
            DataType::Row(r) => r.fields.iter().any(|f| self.nc_ordered(&f.data_type, deeper)),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Reference(_)
            | DataType::Json
            | DataType::Extension(_) => false,
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.66] Drops the ordering of `udt`: with
    /// CASCADE, each routine, view, constraint, assertion and trigger that
    /// compares its values [GR1 to GR6]; its form NONE again [GR7].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has none \[SR2\], or, not `cascade`, something
    /// compares its values \[SR4\].
    pub(in crate::db) fn drop_ordering(&mut self, udt: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.ordering(udt).is_none() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.66 SR2] type {udt} has no ordering")));
        }
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.remove_ordering(udt);
        after.catalog = Arc::from(catalog);
        self.drop_unbinding(&after, cascade, &format!("the ordering of {udt}"), "§11.66 SR4")
    }
}

/// The specific name of the function of `c`, if it has one.
fn category_function(c: &OrderingCategory) -> Option<QualifiedName> {
    match c {
        OrderingCategory::Relative(f) | OrderingCategory::Map(f) => Some(f.clone()),
        OrderingCategory::State(_) => None,
    }
}

/// [ISO/IEC 9075-2:2023 §11.65 SR6b, SR7, SR8] That `f` is a function the
/// category `category` takes of the type `data_type` of form `form`:
/// deterministic, modifying no SQL-data; RELATIVE's a regular function of
/// two parameters of the type returning INTEGER; MAP's of one parameter of
/// the type returning a predefined type, of FULL no large object one.
fn ordering_function(f: &SqlRoutineDesc, category: &Written, (data_type, form): (&DataType, OrderingForm)) -> Result<()> {
    let refused = |why: &str| Err(rule(format!("[ISO/IEC 9075-2:2023 §11.65 {why}, as {} is not", f.name)));
    if !f.deterministic || f.access == DataAccess::Modifies {
        return refused("SR6b] an ordering function is deterministic and modifies no SQL-data");
    }
    let returns = f.returns.clone().unwrap_or(DataType::Boolean);
    if let Written::Relative(_) = category {
        let integer = matches!(returns, DataType::Exact { scale: 0, spelled: Spelled::Integer, .. });
        if f.method.is_some() || f.parameter_types() != [data_type.clone(), data_type.clone()] || !integer {
            return refused(&format!("SR7] a RELATIVE function is a regular function of two parameters of {data_type} returning INTEGER"));
        }
        return Ok(());
    }
    let predefined = !user_defined(&returns)
        && !matches!(returns, DataType::Row(_) | DataType::Array(_) | DataType::Multiset(_) | DataType::Extension(_));
    if f.parameter_types() != [data_type.clone()] || !predefined {
        return refused(&format!("SR8] a MAP function has one parameter, of {data_type}, and returns a predefined type"));
    }
    if form == OrderingForm::Full && returns.is_lob() {
        return refused("SR8, §9.14 SR3] the MAP function of an ordering FULL returns no large object string");
    }
    Ok(())
}
