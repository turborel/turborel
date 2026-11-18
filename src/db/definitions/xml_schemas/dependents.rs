//! What holds an XML type of XMLSCHEMA of a registered XML Schema
//! [ISO/IEC 9075-14:2006 draft §4.2.2, §6.1 GR1d], for
//! `State::xml_schema_dependents`: the tables, domains and user-defined
//! types of such a column, data type, source or attribute, which nothing
//! drops; and the SQL-invoked routines of such a parameter or result,
//! which DROP ... CASCADE drops, with what invokes them.

use super::super::super::State;
use super::super::super::revoke::Abandoned;
use crate::catalog::QualifiedName;
use crate::value::DataType;
use crate::value::types::MAX_TYPE_NESTING;

/// Whether `t` is, or holds, an XML type of XMLSCHEMA of one of the
/// registered XML Schemas `gone`: as itself, an element, a field or a
/// distinct type's source, `depth` types down, no deeper than types nest.
fn names_any(t: &DataType, gone: &[QualifiedName], depth: usize) -> bool {
    if depth > MAX_TYPE_NESTING {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match t {
        DataType::Xml(x) => x.schema().is_some_and(|n| gone.iter().any(|g| g.auth == n.schema && g.ident == n.name)),
        DataType::Array(a) => names_any(&a.element, gone, deeper),
        DataType::Multiset(m) => names_any(&m.element, gone, deeper),
        DataType::Row(r) => r.fields.iter().any(|f| names_any(&f.data_type, gone, deeper)),
        DataType::Distinct(d) => names_any(&d.source, gone, deeper),
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
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => false,
    }
}

impl State {
    /// The tables, domains and user-defined types of the schemas `within`
    /// takes that hold an XML type of XMLSCHEMA of one of `gone`: a
    /// column's type, a domain's data type, a distinct type's source, a
    /// structured type's attribute's type.
    pub(super) fn typed_by_xml_schemas(&self, gone: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Vec<QualifiedName> {
        let catalog = &*self.catalog;
        let of = |t: &DataType| names_any(t, gone, 0);
        let tables = catalog.tables().into_iter().filter(|t| t.columns.iter().any(|c| of(&c.data_type))).map(|t| t.name.clone());
        let domains = catalog.domains().into_iter().filter(|d| of(&d.data_type)).map(|d| d.name.clone());
        let distinct = catalog.user_types().into_iter().filter(|d| of(&d.source)).map(|d| d.name.clone());
        let structured =
            catalog.structured_types().into_iter().filter(|s| s.attributes.iter().any(|a| of(&a.data_type))).map(|s| s.name.clone());
        tables.chain(domains).chain(distinct).chain(structured).filter(|n| within(&n.auth)).collect()
    }

    /// The SQL-invoked routines of the schemas `within` takes of a
    /// parameter or result of an XML type of XMLSCHEMA of one of `gone`,
    /// each routine that invokes one of them, at any remove, and what
    /// else invokes one, as DROP ROUTINE ... CASCADE takes them [ISO/IEC
    /// 9075-2:2023 §11.62 GR1].
    pub(super) fn routines_typed_by_xml_schemas(&self, gone: &[QualifiedName], within: &dyn Fn(&str) -> bool) -> Abandoned {
        let of = |t: &DataType| names_any(t, gone, 0);
        let typed: Vec<QualifiedName> = (self.catalog.routines().into_iter())
            .filter(|r| within(&r.specific.auth) && (r.parameters.iter().any(|p| of(&p.data_type)) || r.returns.as_ref().is_some_and(of)))
            .map(|r| r.specific.clone())
            .collect();
        if typed.is_empty() {
            return Abandoned::default();
        }
        let routines = self.invokers_of(&typed);
        Abandoned { routines: routines.clone(), ..self.invoking(&routines) }
    }
}
