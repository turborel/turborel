//! [ISO/IEC 9075:1992 §11.21, §6.10 GR13] A domain's constraints bound
//! over a value alone: over a table of one column VALUE made for them in
//! the domain's schema, as each copy on a column is bound, and as a CAST to
//! the domain checks them. Each is of the subject routines it fixed as it
//! was defined [ISO/IEC 9075-2:2023 §4.35.2] (RULES.md D73).

use std::sync::Arc;

use super::super::State;
use super::on_column;
use crate::binder::{Invoked, Referencing};
use crate::catalog::{ColumnDesc, ConstraintKind, DomainDesc, Horizon, QualifiedName, TableDesc};
use crate::error::Result;
use crate::logical::Cond;
use crate::parser::VALUE_COLUMN;
use crate::value::DataType;

impl State {
    /// The tables a domain constraint's condition `text`, its names of
    /// schema `names`, of the subject routines `horizon` holds, reads,
    /// bound over a column VALUE of `data_type` of a table of the domain's
    /// schema, that is no other: as each copy of it will be bound; and the
    /// horizon of the routines it invokes and the registered XML Schemas it
    /// indicates [ISO/IEC 9075-14:2006 draft §4.2.6].
    pub(in crate::db) fn bound_on_value(
        &self,
        domain: &QualifiedName,
        data_type: DataType,
        (text, names, horizon): (&str, &str, &Horizon),
    ) -> Result<(Vec<QualifiedName>, Horizon)> {
        let invoked = Invoked::default();
        let (cond, probe) = self.on_value(domain, data_type, (text, names, horizon), Some(&invoked))?;
        let seen = Horizon::default().invoking(invoked.take()).indicating(invoked.take_xml_schemas());
        Ok((cond.tables().into_iter().filter(|t| *t != probe).collect(), seen))
    }

    /// [ISO/IEC 9075:1992 §6.10 GR13] The constraints of `domain`, each
    /// bound over a row of a value alone, VALUE its one column: what a CAST
    /// to it checks [§4.10.3].
    ///
    /// # Errors
    ///
    /// A condition no longer binds.
    pub(crate) fn cast_checks(&self, domain: &DomainDesc) -> Result<Vec<(QualifiedName, Cond)>> {
        let mut out = Vec::new();
        for c in &domain.constraints {
            let ConstraintKind::Check { text, names, horizon, .. } = &c.kind else { continue };
            out.push((c.name.clone(), self.on_value(&domain.name, domain.data_type.clone(), (text, names, horizon), None)?.0));
        }
        Ok(out)
    }

    /// The domain constraint `text` of `domain`, its names of schema
    /// `names`, of the subject routines `horizon` holds, bound over the one
    /// column VALUE, of `data_type`, of a table of the domain's schema made
    /// for it, the routines it invokes noted in `invoked`, if given; and
    /// that table's name.
    fn on_value(
        &self,
        domain: &QualifiedName,
        data_type: DataType,
        (text, names, horizon): (&str, &str, &Horizon),
        invoked: Option<&Invoked>,
    ) -> Result<(Cond, QualifiedName)> {
        let (staged, probe) = self.with_value(domain, data_type);
        let seen =
            Horizon { widths: vec![(probe.clone(), 1)], routines: horizon.routines.clone(), xml_schemas: horizon.xml_schemas.clone() };
        let on_probe = Referencing::of_value(&probe);
        let cond = staged.bind_check(&probe, (&on_column(text, VALUE_COLUMN), names, &seen), (None, &on_probe, invoked))?;
        Ok((cond, probe))
    }

    /// [ISO/IEC 9075:1992 §11.21 GR4] The columns the domain constraint
    /// `text` of `domain`, its names of schema `names`, references, bound
    /// as `on_value` binds it.
    ///
    /// # Errors
    ///
    /// The condition no longer binds.
    pub(in crate::db) fn domain_constraint_uses(
        &self,
        domain: &DomainDesc,
        (text, names, horizon): (&str, &str, &Horizon),
    ) -> Result<Vec<(QualifiedName, String)>> {
        let (staged, probe) = self.with_value(&domain.name, domain.data_type.clone());
        let seen =
            Horizon { widths: vec![(probe.clone(), 1)], routines: horizon.routines.clone(), xml_schemas: horizon.xml_schemas.clone() };
        staged.value_uses(&probe, (&on_column(text, VALUE_COLUMN), names, &seen))
    }

    /// This state with a table of `domain`'s schema made for its constraints:
    /// of one column VALUE, of `data_type`; and that table's name.
    fn with_value(&self, domain: &QualifiedName, data_type: DataType) -> (State, QualifiedName) {
        let column = ColumnDesc {
            name: VALUE_COLUMN.to_owned(),
            data_type,
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        };
        let probe = QualifiedName { auth: domain.auth.clone(), ident: format!("{} VALUE", domain.ident) };
        let desc = TableDesc {
            periods: Vec::new(),
            typed: None,
            name: probe.clone(),
            columns: vec![column],
            constraints: Vec::new(),
            engine: String::new(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        let mut staged = State::clone(self);
        let mut catalog = staged.catalog.duplicate();
        catalog.add_table(desc);
        staged.catalog = Arc::from(catalog);
        (staged, probe)
    }
}
