//! What a routine dropped may be to a user-defined type [ISO/IEC
//! 9075-2:2023 §11.62 SR4a, SR4d, GR2 to GR4]: the ordering function of
//! a type, a function of a transform group, or a cast function. RESTRICT
//! drops none of them; CASCADE drops the ordering, the transform or the
//! cast with it.

use super::super::ddl::Dropped;
use super::super::{State, rule};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::value::DataType;

/// What a function is to a user-defined type.
enum Use {
    /// The ordering function of the type.
    Ordering(QualifiedName),
    /// A function of the transform group of this name of the type.
    Transform(QualifiedName, String),
    /// The cast function of the cast from the first type to the second.
    Cast(DataType, DataType),
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.62 SR4a, SR4d, GR2 to GR4] Of the routine
    /// of specific name `specific`, dropped: with `cascade` each ordering,
    /// transform and cast it is the function of, dropped with CASCADE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if it is the function of one.
    pub(in crate::db) fn drop_function_uses(&mut self, specific: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let uses = self.function_uses(specific);
        let mut dropped = Dropped::default();
        for u in uses {
            dropped.absorb(match u {
                Use::Ordering(udt) if cascade => self.drop_ordering(&udt, true)?,
                Use::Transform(udt, group) if cascade => self.drop_transform(&udt, Some(&group), true)?,
                Use::Cast(source, target) if cascade => self.drop_cast(&source, &target, true)?,
                Use::Ordering(udt) => {
                    return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.62 SR4a] {specific} is the ordering function of {udt}: drop it with CASCADE")));
                }
                Use::Transform(udt, group) => {
                    let msg = format!("[ISO/IEC 9075-2:2023 §11.62 SR4d i] {specific} is a function of transform group {group} of {udt}: drop it with CASCADE");
                    return Err(rule(msg));
                }
                Use::Cast(source, target) => {
                    return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.62 SR4d ii] {specific} is the cast function from {source} to {target}: drop it with CASCADE")));
                }
            });
        }
        Ok(dropped)
    }

    /// Each ordering, transform group and cast whose function is the
    /// routine of specific name `specific`.
    fn function_uses(&self, specific: &QualifiedName) -> Vec<Use> {
        let catalog = &*self.catalog;
        let mut uses: Vec<Use> =
            catalog.orderings().into_iter().filter(|o| o.function() == Some(specific)).map(|o| Use::Ordering(o.udt.clone())).collect();
        let types =
            catalog.structured_types().into_iter().map(|t| t.name.clone()).chain(catalog.user_types().into_iter().map(|t| t.name.clone()));
        for udt in types {
            let groups =
                catalog.transforms(&udt).iter().filter(|g| g.to_sql.as_ref() == Some(specific) || g.from_sql.as_ref() == Some(specific));
            uses.extend(groups.map(|g| Use::Transform(udt.clone(), g.name.clone())).collect::<Vec<_>>());
        }
        uses.extend(catalog.casts().into_iter().filter(|c| c.function == *specific).map(|c| Use::Cast(c.source.clone(), c.target.clone())));
        uses
    }
}
