//! [ISO/IEC 9075:1992 §18.1; ISO/IEC 9075-2:2023 §23.1] GET DIAGNOSTICS
//! reading a diagnostics area: the session's, or that of a function whose
//! body runs where a query evaluates it (`db/routines/program/executor.rs`).

use super::all;
use super::{Area, Condition, Shown, all_target, number, typed};
use crate::ast::{AllQualifier, ConditionItem, DiagnosticsInformation, GetDiagnostics, StatementItem, ValueSpecification};
use crate::binder;
use crate::catalog::{Principal, QualifiedName, applicable, owner_of};
use crate::db::assignment::assign_targets;
use crate::db::{Completion, State};
use crate::error::{Result, Subject};
use crate::params::Params;
use crate::settings::CATALOG_NAME;
use crate::value::{DataType, Value};

/// What GET DIAGNOSTICS reads an area as: of the database `st`, by `user`,
/// whose privileges say which names a condition gives it [ISO/IEC
/// 9075:1992 §18.1 GR3j], while a transaction is `active` or not.
pub(in crate::db) struct Reader<'r> {
    pub(in crate::db) st: &'r State,
    pub(in crate::db) user: Principal<'r>,
    pub(in crate::db) active: bool,
}

impl Reader<'_> {
    /// [ISO/IEC 9075:1992 §18.1; ISO/IEC 9075-2:2023 §23.1] GET DIAGNOSTICS
    /// `get` of `area`: what it holds assigned to its targets.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an item cannot be assigned to its target \[SR1\],
    /// ALL's target is not a character string \[SR2\], or the condition
    /// number is not exact with scale 0 \[SR3\]; -301, SQLSTATE 35000, if
    /// no condition has that number \[GR3\]; 22001 if ALL's string is
    /// longer than a string may be.
    pub(in crate::db) fn get(&self, area: &Area, get: &GetDiagnostics, params: &mut Params) -> Result<Completion> {
        let statement = |item: StatementItem| area.value(item, self.active);
        let (targets, values) = match &get.information {
            DiagnosticsInformation::Statement(items) => typed(items.iter().map(|(t, item)| (t, item.numeric(), statement(*item))), params)?,
            DiagnosticsInformation::Condition { number, items } => {
                let (n, condition) = area.numbered(number, &|v| self.valued(v, params))?;
                let shown = self.shown(condition);
                typed(items.iter().map(|(t, item)| (t, item.numeric(), shown.value(*item, (n, condition)))), params)?
            }
            DiagnosticsInformation::All { target, qualifier } => {
                let presented = self.presented(area, qualifier, params)?;
                (vec![all_target(target, params)?], vec![all::concatenated(presented)?])
            }
        };
        // [X3.135-1986 §8.6 GR8-GR10; ISO/IEC 9075:1992 §9.1] as a FETCH
        // assigns: a string cut is a warning, though the area is left as
        // it was [§18.1 GR1a].
        assign_targets(&values, &targets, params, "X3.135-1986 §8.6 GR8", "X3.135-1986 §8.6 GR10")
    }

    /// The value and type of the `<condition number>` or other simple value
    /// `v`, of `params`.
    fn valued(&self, v: &ValueSpecification, params: &Params) -> Result<(Value, DataType)> {
        binder::value_specification(v, self.user, params)
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR6a] The items `<all information>`
    /// presents, by name, of `area`, as `qualifier` asks: the statement's,
    /// each condition's preceded by its CONDITION_NUMBER, or those of the
    /// one condition its number names, without it.
    fn presented(&self, area: &Area, qualifier: &AllQualifier, params: &Params) -> Result<Vec<(&'static str, Value)>> {
        let mut presented = Vec::new();
        if let AllQualifier::Everything | AllQualifier::Statement = qualifier {
            presented.extend(StatementItem::EVERY.map(|item| (item.name(), area.value(item, self.active))));
        }
        let conditions = match qualifier {
            AllQualifier::Statement => Vec::new(),
            AllQualifier::Everything | AllQualifier::Condition(None) => area.conditions().map(|(n, c)| (true, n, c)).collect(),
            AllQualifier::Condition(Some(number)) => {
                let (n, condition) = area.numbered(number, &|v| self.valued(v, params))?;
                vec![(false, n, condition)]
            }
        };
        for (numbered, n, condition) in conditions {
            if numbered {
                presented.push((ConditionItem::ConditionNumber.name(), number(n)));
            }
            let shown = self.shown(condition);
            let items = ConditionItem::EVERY.into_iter().filter(|item| *item != ConditionItem::ConditionNumber);
            presented.extend(items.map(|item| (item.name(), shown.value(item, (n, condition)))));
        }
        Ok(presented)
    }

    /// The names `condition` gives, as the reader may see them [ISO/IEC
    /// 9075:1992 §18.1 GR3f–GR3j].
    fn shown(&self, condition: &Condition) -> Shown {
        let st = self.st;
        let catalog = st.settings.get(CATALOG_NAME).to_string().trim_end().to_owned();
        // [GR3j] but for a syntax error or access rule violation, a table
        // on which the identifier holds no privilege, and a constraint of
        // one, are not named.
        let access = condition.state.is_syntax_or_access();
        let visible = |(schema, ident): &(String, String)| {
            let name = QualifiedName { auth: schema.clone(), ident: ident.clone() };
            access || !applicable(&*st.catalog, self.user.cell, &name).0.is_empty()
        };
        let Subject { constraint, table, column, cursor, assertion, trigger, routine, signalled } = &condition.subject;
        let named = table.is_some();
        let table = table.clone().filter(visible);
        // [GR3j iii] a table constraint goes with its table; [GR3j iv] an
        // assertion is shown to its schema's owner.
        let shown_constraint = |(schema, _): &(String, String)| {
            if *assertion { self.user.cell.includes(owner_of(&*st.catalog, schema)) } else { !named || table.is_some() }
        };
        let constraint = constraint.clone().filter(shown_constraint);
        let column = column.clone().filter(|_| table.is_some());
        let (cursor, trigger, routine, signalled) = (cursor.clone(), trigger.clone(), routine.clone(), signalled.clone());
        Shown { catalog, constraint, table, column, cursor, trigger, routine, signalled }
    }
}
